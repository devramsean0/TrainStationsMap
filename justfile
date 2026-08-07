default:
    @just --list

# Run pre-commit hooks on all files, including autoformatting
pre-commit-all:
    pre-commit run --all-files

# Run 'cargo run' on the project
run *ARGS:
    cargo run {{ARGS}}

# Run 'bacon' to run the project (auto-recompiles)
watch *ARGS:
	bacon --job run -- -- {{ ARGS }}

# Install frontend dependencies
frontend-install:
    cd frontend && npm install

# Build the frontend (run frontend-install first if needed)
frontend-build:
    cd frontend && npm run build

# Run the Vite dev server (proxies /api to localhost:3000)
frontend-dev:
    cd frontend && npm run dev
