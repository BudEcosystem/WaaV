#!/usr/bin/env python3
"""
HTTPS server for Bud Foundry Dashboard

This enables microphone access from other devices on the network.
"""

import http.server
import ssl
import os
import sys

PORT = 8443
CERT_FILE = 'cert.pem'
KEY_FILE = 'key.pem'

def main():
    # Change to dashboard directory
    os.chdir(os.path.dirname(os.path.abspath(__file__)))

    if not os.path.exists(CERT_FILE) or not os.path.exists(KEY_FILE):
        print(f"Error: Certificate files not found ({CERT_FILE}, {KEY_FILE})")
        print("Generate them with:")
        print("  openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 365 -nodes")
        sys.exit(1)

    handler = http.server.SimpleHTTPRequestHandler

    server = http.server.HTTPServer(('0.0.0.0', PORT), handler)

    # Create SSL context
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(CERT_FILE, KEY_FILE)

    server.socket = context.wrap_socket(server.socket, server_side=True)

    print(f"HTTPS Dashboard running on:")
    print(f"  https://localhost:{PORT}")

    # Try to get local IP
    import socket
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        local_ip = s.getsockname()[0]
        s.close()
        print(f"  https://{local_ip}:{PORT}")
    except:
        pass

    print("\nNote: Your browser will show a security warning for the self-signed certificate.")
    print("Click 'Advanced' -> 'Proceed to site' to continue.\n")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nServer stopped.")

if __name__ == '__main__':
    main()
