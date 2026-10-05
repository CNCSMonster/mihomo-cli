#!/usr/bin/env python3
"""
Mock 订阅服务器
返回 Clash 格式的订阅配置
"""
import http.server
import socketserver
import sys

MOCK_SUBSCRIPTION = """
mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
proxies:
  - name: "Mock Proxy 1"
    type: ss
    server: 127.0.0.1
    port: 8388
    cipher: aes-256-gcm
    password: "password"
  - name: "Mock Proxy 2"
    type: vmess
    server: 127.0.0.1
    port: 10002
    uuid: uuid
    alterId: 0
    cipher: auto
proxy-groups:
  - name: "Proxy"
    type: select
    proxies:
      - "Mock Proxy 1"
      - "Mock Proxy 2"
rules:
  - MATCH,Proxy
"""

class MockSubHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if '/subscription' in self.path or '/sub' in self.path:
            self.send_response(200)
            self.send_header('Content-type', 'text/yaml')
            self.send_header('Content-Disposition', 'attachment; filename="config.yaml"')
            self.end_headers()
            self.wfile.write(MOCK_SUBSCRIPTION.encode())
        elif '/health' in self.path:
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b'OK')
        else:
            self.send_response(404)
            self.end_headers()
    
    def log_message(self, format, *args):
        # 静默日志
        pass

def run_server(port=8081):
    with socketserver.TCPServer(('', port), MockSubHandler) as httpd:
        print(f'Mock subscription server running on port {port}')
        httpd.serve_forever()

if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8081
    run_server(port)
