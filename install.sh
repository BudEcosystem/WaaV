#!/bin/bash
# WaaV - AI Gateway Installer
# One-command installation script for WaaV Gateway

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATEWAY_DIR="$SCRIPT_DIR/gateway"

echo "============================================"
echo "  WaaV Gateway Installer"
echo "============================================"
echo ""

# Check for required tools
check_dependencies() {
    local missing=()

    if ! command -v curl &> /dev/null; then
        missing+=("curl")
    fi

    if ! command -v gcc &> /dev/null && ! command -v clang &> /dev/null; then
        missing+=("gcc or clang (C compiler)")
    fi

    if [ ${#missing[@]} -ne 0 ]; then
        echo "Missing required dependencies: ${missing[*]}"
        echo "Please install them and run this script again."
        exit 1
    fi
}

# Install Rust if not present
install_rust() {
    if ! command -v cargo &> /dev/null; then
        echo "Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
        echo "Rust installed successfully."
    else
        echo "Rust is already installed: $(rustc --version)"
    fi
}

# Build the gateway
build_gateway() {
    echo ""
    echo "Building WaaV Gateway (release mode)..."
    cd "$GATEWAY_DIR"
    cargo build --release
    echo "Build completed successfully."
}

# Install binary to system
install_binary() {
    echo ""
    echo "Installing waav-gateway to /usr/local/bin..."

    if [ -w /usr/local/bin ]; then
        cp "$GATEWAY_DIR/target/release/waav-gateway" /usr/local/bin/
    else
        sudo cp "$GATEWAY_DIR/target/release/waav-gateway" /usr/local/bin/
    fi

    echo "Binary installed."
}

# Create default configuration
setup_config() {
    local config_dir="/etc/waav-gateway"
    local config_file="$config_dir/config.yaml"

    if [ ! -f "$config_file" ]; then
        echo ""
        echo "Creating default configuration..."

        if [ -w "$(dirname "$config_dir")" ]; then
            mkdir -p "$config_dir"
            cp "$GATEWAY_DIR/config.example.yaml" "$config_file"
        else
            sudo mkdir -p "$config_dir"
            sudo cp "$GATEWAY_DIR/config.example.yaml" "$config_file"
        fi

        echo "Configuration created at $config_file"
        echo "Edit this file to add your API keys and customize settings."
    else
        echo "Configuration already exists at $config_file"
    fi
}

# Print success message
print_success() {
    echo ""
    echo "============================================"
    echo "  Installation Complete!"
    echo "============================================"
    echo ""
    echo "To start WaaV Gateway:"
    echo "  waav-gateway -c /etc/waav-gateway/config.yaml"
    echo ""
    echo "Or run directly from source:"
    echo "  cd $GATEWAY_DIR && cargo run --release"
    echo ""
    echo "Quick test (no config required):"
    echo "  waav-gateway"
    echo ""
    echo "For TLS support, set these environment variables:"
    echo "  TLS_ENABLED=true"
    echo "  TLS_CERT_PATH=/path/to/cert.pem"
    echo "  TLS_KEY_PATH=/path/to/key.pem"
    echo ""
}

# Main installation flow
main() {
    check_dependencies
    install_rust
    build_gateway
    install_binary
    setup_config
    print_success
}

main "$@"
