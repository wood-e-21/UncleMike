dev:
    npm run dev

build:
    npm run build

test:
    cargo test -p mike-backend --no-default-features --features local-storage

typecheck:
    npm run build:electron

codegen:
    echo "codegen not yet implemented, see PLAN.md"

package:
    npm run dist:dir
