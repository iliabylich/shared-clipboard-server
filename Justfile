server:
    RUST_LOG=trace cargo run --bin shared-clipboard-server -- --start

generate-config:
    RUST_LOG=trace cargo run --bin shared-clipboard-server -- --generate

client name url token:
    RUST_LOG=trace cargo run --bin shared-clipboard-test-client -- {{name}} "{{url}}" "{{token}}"

build-deb-packages:
    cargo deb --deb-revision="$(date +%s)" -p shared-clipboard-server
    cargo deb --deb-revision="$(date +%s)" -p shared-clipboard-test-client
