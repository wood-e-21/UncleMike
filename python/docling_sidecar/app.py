"""
Mike Local — Docling sidecar.

FastAPI/Uvicorn server that wraps IBM Docling's DocumentConverter +
HybridChunker for the Mike Electron app. Spawned next to the Express
backend; binds 127.0.0.1 on an OS-assigned port and writes that port to
the runtime file so Mike's Node backend can discover it.

Endpoints:
    GET  /health   → {ok, models_loaded, parser_version}
    GET  /version  → {version, schema_version, capabilities}
    POST /parse    → DoclingDocument JSON + Markdown + tables + chunks
    POST /rechunk  → re-chunk a stored DoclingDocument JSON

Configuration via env vars. Universal envelope per docs/03-sidecars.md:
    MIKE_SIDECAR_NAME       — "docling"
    MIKE_SIDECAR_RUNTIME    — path for {port,pid,version,...} runtime JSON
    MIKE_SIDECAR_CACHE_DIR  — model + artifact cache root
    MIKE_SIDECAR_LOG        — sidecar log file path (Phase 2)
    MIKE_SIDECAR_PARENT_PID — parent PID for the watchdog thread

Sidecar-specific (still under the MIKE_DOCLING_ prefix because they
configure Docling internals, not the supervisor protocol):
    MIKE_DOCLING_MAX_TOKENS — HybridChunker max_tokens (default 1024)
    MIKE_DOCLING_DEVICE     — 'mps' | 'cpu' | 'cuda' (default 'mps' on darwin)

Legacy MIKE_DOCLING_RUNTIME / MIKE_DOCLING_CACHE_DIR / MIKE_DOCLING_PARENT_PID
are still honored as a fallback during the Phase 1 → Phase 3 transition;
when the Rust supervisor takes over spawning, only the MIKE_SIDECAR_*
names will be set.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
import signal
import socket
import sys
import tempfile
import threading
from pathlib import Path
from typing import Any

import uvicorn
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

logger = logging.getLogger("docling_sidecar")

# ----- Lazy Docling imports -------------------------------------------------
# Loading docling pulls in torch + transformers + the layout/table models;
# that's hundreds of MB and ~10-30s on a cold machine. Lazy-load on first
# /parse so /health responds immediately and the parent process can detect
# the sidecar is up.

_converter: Any = None
_chunker: Any = None
_parser_version: str | None = None
_models_loaded = False
_load_lock = threading.Lock()


def _detect_device() -> str:
    forced = os.environ.get("MIKE_DOCLING_DEVICE")
    if forced:
        return forced
    if sys.platform == "darwin":
        return "mps"
    return "cpu"


def _ensure_loaded() -> None:
    """Load DocumentConverter + HybridChunker exactly once. Thread-safe."""
    global _converter, _chunker, _parser_version, _models_loaded
    if _models_loaded:
        return
    with _load_lock:
        if _models_loaded:
            return
        logger.info("Loading Docling models (this can take 10–30 seconds on first run)...")

        from docling.document_converter import DocumentConverter, PdfFormatOption
        from docling.datamodel.base_models import InputFormat
        from docling.datamodel.pipeline_options import (
            AcceleratorOptions,
            PdfPipelineOptions,
            TableFormerMode,
            TableStructureOptions,
        )
        # OcrAutoOptions picks the first available OCR backend in the order
        # rapidocr-onnxruntime → easyocr → rapidocr-torch. Lets us avoid
        # hard-pinning to a specific engine and lets the user install
        # whichever one fits their machine.
        try:
            from docling.datamodel.pipeline_options import OcrAutoOptions
            ocr_opts: object = OcrAutoOptions()
        except Exception:
            from docling.datamodel.pipeline_options import EasyOcrOptions
            ocr_opts = EasyOcrOptions(lang=["en"])
        from docling.chunking import HybridChunker
        from docling_core.transforms.chunker.tokenizer.huggingface import (
            HuggingFaceTokenizer,
        )

        # MIKE_DOCLING_CACHE_DIR is treated as a HuggingFace cache override
        # so Docling's default model loader downloads models there on first
        # use. Setting `artifacts_path` directly would tell Docling "models
        # are already here" and it would NOT auto-download, which fails on
        # an empty workspace cache. Redirecting HF_HOME / TRANSFORMERS_CACHE
        # gives us workspace-isolated model storage AND lets Docling do its
        # own download on first run.
        cache_dir = os.environ.get("MIKE_SIDECAR_CACHE_DIR") or os.environ.get(
            "MIKE_DOCLING_CACHE_DIR"
        )
        if cache_dir:
            cache_path = Path(cache_dir).expanduser().resolve()
            cache_path.mkdir(parents=True, exist_ok=True)
            os.environ.setdefault("HF_HOME", str(cache_path / "huggingface"))
            os.environ.setdefault(
                "TRANSFORMERS_CACHE", str(cache_path / "huggingface" / "transformers")
            )

        pdf_opts = PdfPipelineOptions(
            do_ocr=True,
            do_table_structure=True,
            table_structure_options=TableStructureOptions(
                mode=TableFormerMode.ACCURATE,
                do_cell_matching=True,
            ),
            ocr_options=ocr_opts,
            accelerator_options=AcceleratorOptions(device=_detect_device()),
            document_timeout=120.0,
            generate_page_images=False,
        )

        _converter = DocumentConverter(
            format_options={
                InputFormat.PDF: PdfFormatOption(pipeline_options=pdf_opts),
            }
        )

        max_tokens = int(os.environ.get("MIKE_DOCLING_MAX_TOKENS", "1024"))
        tokenizer = HuggingFaceTokenizer.from_pretrained(
            model_name="sentence-transformers/all-MiniLM-L6-v2",
            max_tokens=max_tokens,
        )
        _chunker = HybridChunker(tokenizer=tokenizer, merge_peers=True)

        try:
            from importlib.metadata import version as _pkg_version

            _parser_version = _pkg_version("docling")
        except Exception:
            _parser_version = "unknown"
        _models_loaded = True
        logger.info("Docling models loaded (parser_version=%s)", _parser_version)


# ----- Format detection ------------------------------------------------------

EXTENSION_TO_FORMAT = {
    ".pdf": "PDF",
    ".docx": "DOCX",
    # Legacy .doc binary is NOT supported here. Mike's upload pipeline keeps
    # .doc source bytes and creates a separate PDF rendition for display;
    # callers should hand us the PDF rendition path for .doc inputs.
    ".xlsx": "XLSX",
    ".pptx": "PPTX",
    ".csv": "CSV",
    ".html": "HTML",
    ".htm": "HTML",
    ".md": "MD",
    ".markdown": "MD",
    ".png": "IMAGE",
    ".jpg": "IMAGE",
    ".jpeg": "IMAGE",
    ".tiff": "IMAGE",
    ".tif": "IMAGE",
    ".bmp": "IMAGE",
    ".webp": "IMAGE",
}


def _detect_input_format(file_path: str, hint_format: str | None) -> str:
    if hint_format:
        return hint_format.upper()
    ext = Path(file_path).suffix.lower()
    return EXTENSION_TO_FORMAT.get(ext, "UNKNOWN")


def _prepare_processing_path(file_path: str, fmt: str) -> tuple[str, str | None]:
    """Docling infers format from the file extension. If the path has no
    suffix (Mike's storage keys often don't), copy it to a tempfile with the
    right extension and return (temp_path, cleanup_path). Otherwise return
    (file_path, None).
    """
    desired_ext = {
        "PDF": ".pdf",
        "DOCX": ".docx",
        "XLSX": ".xlsx",
        "PPTX": ".pptx",
        "CSV": ".csv",
        "HTML": ".html",
        "MD": ".md",
        "IMAGE": ".png",
    }.get(fmt)
    actual = Path(file_path)
    if desired_ext is None:
        return file_path, None
    if actual.suffix.lower() == desired_ext:
        return file_path, None
    with tempfile.NamedTemporaryFile(delete=False, suffix=desired_ext) as tmp:
        tmp_path = Path(tmp.name)
    shutil.copyfile(actual, tmp_path)
    return str(tmp_path), str(tmp_path)


# ----- HTTP models ----------------------------------------------------------


class ParseRequest(BaseModel):
    file_path: str = Field(..., description="Local filesystem path")
    hint_format: str | None = Field(None, description="PDF|DOCX|HTML|MD|IMAGE")


class ParsedTable(BaseModel):
    table_index: int
    page_no: int | None
    headings: list[str]
    markdown: str
    html: str


class ParsedChunk(BaseModel):
    chunk_index: int
    text: str
    page_no: int | None
    headings: list[str]
    labels: list[str]
    is_table: bool
    token_count: int | None


class ParseResponse(BaseModel):
    input_format: str
    parser_version: str
    page_count: int
    docling_json: dict[str, Any]
    markdown: str
    tables: list[ParsedTable]
    chunks: list[ParsedChunk]


class RechunkRequest(BaseModel):
    docling_json: dict[str, Any]
    max_tokens: int | None = None


class RechunkResponse(BaseModel):
    chunks: list[ParsedChunk]


class HealthResponse(BaseModel):
    ok: bool
    models_loaded: bool
    parser_version: str | None


class VersionResponse(BaseModel):
    """Wire shape required by docs/03-sidecars.md. The supervisor in
    `backend/src/sidecars/supervisor.rs` parses `version` for major-
    version compatibility checks."""

    version: str
    schema_version: int
    capabilities: list[str]


# Bump the second component (minor) when adding capabilities; bump the
# first (major) only on a breaking wire-format change in /parse or
# /rechunk. The backend's expected_major_version() must match.
SIDECAR_VERSION = "1.0.0"
SIDECAR_SCHEMA_VERSION = 1
SIDECAR_CAPABILITIES = ["parse", "rechunk"]


# ----- FastAPI app ----------------------------------------------------------

app = FastAPI(title="mike-docling-sidecar")


@app.get("/health", response_model=HealthResponse)
def health() -> HealthResponse:
    return HealthResponse(
        ok=True, models_loaded=_models_loaded, parser_version=_parser_version
    )


@app.get("/version", response_model=VersionResponse)
def version() -> VersionResponse:
    """Required by docs/03-sidecars.md. The supervisor calls this on
    every probe and refuses to use a sidecar whose major version
    doesn't match the backend's expectation."""
    return VersionResponse(
        version=SIDECAR_VERSION,
        schema_version=SIDECAR_SCHEMA_VERSION,
        capabilities=SIDECAR_CAPABILITIES,
    )


@app.post("/parse", response_model=ParseResponse)
def parse(req: ParseRequest) -> ParseResponse:
    if not Path(req.file_path).exists():
        raise HTTPException(status_code=404, detail=f"File not found: {req.file_path}")
    fmt = _detect_input_format(req.file_path, req.hint_format)
    if fmt == "UNKNOWN":
        raise HTTPException(
            status_code=415,
            detail=f"Unsupported file format for {req.file_path}",
        )

    _ensure_loaded()
    assert _converter is not None and _chunker is not None

    processing_path, cleanup_path = _prepare_processing_path(req.file_path, fmt)
    try:
        result = _converter.convert(processing_path)
    except Exception as e:
        logger.exception("Docling conversion failed for %s", req.file_path)
        raise HTTPException(status_code=500, detail=f"Conversion failed: {e}") from e
    finally:
        if cleanup_path:
            try:
                Path(cleanup_path).unlink(missing_ok=True)
            except Exception:
                pass

    doc = result.document

    # Markdown export with page-break markers — Mike's regex bridge converts
    # these to "[Page N]" / "## Page N" anchors at consumption time.
    try:
        markdown = doc.export_to_markdown(
            page_break_placeholder="<!-- page break -->",
        )
    except TypeError:
        # Older Docling versions without page_break_placeholder kwarg.
        markdown = doc.export_to_markdown()

    # Per-table extraction. doc.tables holds TableItem objects independent
    # of chunking; export_to_markdown(doc) renders each as a span-aware pipe
    # table via TableFormer.
    tables: list[ParsedTable] = []
    try:
        for idx, t in enumerate(getattr(doc, "tables", []) or []):
            page_no = None
            try:
                if t.prov and len(t.prov) > 0:
                    page_no = int(t.prov[0].page_no)
            except Exception:
                pass
            try:
                t_md = t.export_to_markdown(doc)
            except TypeError:
                t_md = t.export_to_markdown()
            try:
                t_html = t.export_to_html(doc)
            except TypeError:
                t_html = t.export_to_html()
            tables.append(
                ParsedTable(
                    table_index=idx,
                    page_no=page_no,
                    headings=[],  # heading breadcrumb populated below if available
                    markdown=t_md,
                    html=t_html,
                )
            )
    except Exception:
        logger.exception("Failed to enumerate doc.tables for %s", req.file_path)

    # Chunk via HybridChunker. Emits one DocChunk per coalesced section.
    chunks: list[ParsedChunk] = []
    table_chunk_idx = 0
    try:
        for ci, ch in enumerate(_chunker.chunk(dl_doc=doc)):
            doc_items = getattr(ch.meta, "doc_items", []) or []
            page_no: int | None = None
            labels: set[str] = set()
            is_table = False
            for di in doc_items:
                try:
                    if not page_no and di.prov:
                        page_no = int(di.prov[0].page_no)
                except Exception:
                    pass
                try:
                    label = getattr(di.label, "value", None) or str(di.label)
                    labels.add(label)
                except Exception:
                    pass
                # TableItem inheritance check via class name (avoid importing the type).
                if di.__class__.__name__ == "TableItem":
                    is_table = True

            text = getattr(ch, "text", "") or ""
            chunks.append(
                ParsedChunk(
                    chunk_index=ci,
                    text=text,
                    page_no=page_no,
                    headings=list(getattr(ch.meta, "headings", None) or []),
                    labels=sorted(labels),
                    is_table=is_table,
                    token_count=len(text.split()),
                )
            )
            # Backfill heading breadcrumb on the first table-chunk for each
            # table — so tabular consumers can show "Schedule A → Pricing".
            if is_table and table_chunk_idx < len(tables):
                tables[table_chunk_idx].headings = list(
                    getattr(ch.meta, "headings", None) or []
                )
                table_chunk_idx += 1
    except Exception as e:
        logger.exception("Chunking failed for %s", req.file_path)
        raise HTTPException(status_code=500, detail=f"Chunking failed: {e}") from e

    return ParseResponse(
        input_format=fmt,
        parser_version=_parser_version or "unknown",
        page_count=len(getattr(doc, "pages", {}) or {}),
        docling_json=doc.export_to_dict(),
        markdown=markdown or "",
        tables=tables,
        chunks=chunks,
    )


@app.post("/rechunk", response_model=RechunkResponse)
def rechunk(req: RechunkRequest) -> RechunkResponse:
    _ensure_loaded()
    from docling_core.types import DoclingDocument

    chunker = _chunker
    if req.max_tokens is not None:
        from docling.chunking import HybridChunker
        from docling_core.transforms.chunker.tokenizer.huggingface import (
            HuggingFaceTokenizer,
        )

        tokenizer = HuggingFaceTokenizer.from_pretrained(
            model_name="sentence-transformers/all-MiniLM-L6-v2",
            max_tokens=req.max_tokens,
        )
        chunker = HybridChunker(tokenizer=tokenizer, merge_peers=True)
    assert chunker is not None

    try:
        doc = DoclingDocument.model_validate(req.docling_json)
    except Exception as e:
        raise HTTPException(
            status_code=400, detail=f"Invalid docling_json: {e}"
        ) from e

    chunks: list[ParsedChunk] = []
    for ci, ch in enumerate(chunker.chunk(dl_doc=doc)):
        doc_items = getattr(ch.meta, "doc_items", []) or []
        page_no: int | None = None
        labels: set[str] = set()
        is_table = False
        for di in doc_items:
            try:
                if not page_no and di.prov:
                    page_no = int(di.prov[0].page_no)
            except Exception:
                pass
            try:
                label = getattr(di.label, "value", None) or str(di.label)
                labels.add(label)
            except Exception:
                pass
            if di.__class__.__name__ == "TableItem":
                is_table = True
        text = getattr(ch, "text", "") or ""
        chunks.append(
            ParsedChunk(
                chunk_index=ci,
                text=text,
                page_no=page_no,
                headings=list(getattr(ch.meta, "headings", None) or []),
                labels=sorted(labels),
                is_table=is_table,
                token_count=len(text.split()),
            )
        )
    return RechunkResponse(chunks=chunks)


# ----- Runtime port discovery ------------------------------------------------


def _bind_port() -> tuple[socket.socket, int]:
    """Bind 127.0.0.1:0 and return (socket, port). Hand the socket to uvicorn."""
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", 0))
    s.listen(128)
    s.setblocking(False)
    return s, s.getsockname()[1]


def _write_runtime(runtime_path: str, port: int) -> None:
    """Write the runtime file per docs/03-sidecars.md. Shape:
        { port, pid, version, schema_version, capabilities, started_at }
    `parser_version` is retained for backward compatibility with the
    Phase-1 Electron supervisor; remove once Phase 3 takes over.
    """
    p = Path(runtime_path)
    p.parent.mkdir(parents=True, exist_ok=True)
    tmp = p.with_suffix(p.suffix + f".{os.getpid()}.tmp")
    tmp.write_text(
        json.dumps(
            {
                "port": port,
                "pid": os.getpid(),
                "version": SIDECAR_VERSION,
                "schema_version": SIDECAR_SCHEMA_VERSION,
                "capabilities": SIDECAR_CAPABILITIES,
                # Legacy:
                "parser_version": _parser_version,
            }
        )
    )
    tmp.replace(p)


def _watch_parent(parent_pid: int) -> None:
    """If the parent process dies (Electron crash, etc.), exit cleanly."""

    def _tick() -> None:
        try:
            os.kill(parent_pid, 0)
        except OSError:
            logger.warning("Parent pid %s gone; exiting.", parent_pid)
            os._exit(0)
        threading.Timer(2.0, _tick).start()

    _tick()


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="[docling-sidecar] %(asctime)s %(levelname)s %(message)s",
    )

    # Prefer the universal MIKE_SIDECAR_RUNTIME (docs/03-sidecars.md);
    # fall back to the legacy MIKE_DOCLING_RUNTIME during Phase 1.
    runtime_path = os.environ.get("MIKE_SIDECAR_RUNTIME") or os.environ.get(
        "MIKE_DOCLING_RUNTIME"
    )
    if not runtime_path:
        logger.error(
            "MIKE_SIDECAR_RUNTIME (or legacy MIKE_DOCLING_RUNTIME) is not set; refusing to start."
        )
        sys.exit(2)

    parent_pid = os.environ.get("MIKE_SIDECAR_PARENT_PID") or os.environ.get(
        "MIKE_DOCLING_PARENT_PID"
    )
    if parent_pid:
        try:
            _watch_parent(int(parent_pid))
        except ValueError:
            pass

    sock, port = _bind_port()
    _write_runtime(runtime_path, port)
    logger.info("Sidecar bound 127.0.0.1:%s; runtime=%s", port, runtime_path)

    config = uvicorn.Config(
        app,
        fd=sock.fileno(),
        log_config=None,
        access_log=False,
        loop="asyncio",
    )
    server = uvicorn.Server(config)

    def _on_sig(_signum: int, _frame: Any) -> None:
        logger.info("Signal received; shutting down.")
        server.should_exit = True

    signal.signal(signal.SIGTERM, _on_sig)
    signal.signal(signal.SIGINT, _on_sig)

    try:
        asyncio.run(server.serve())
    finally:
        try:
            Path(runtime_path).unlink(missing_ok=True)
        except Exception:
            pass


if __name__ == "__main__":
    main()
