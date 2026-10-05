#!/usr/bin/env python3
"""
Mock 代理服务器
支持 HTTP/HTTPS 代理，可模拟各种响应
"""
import http.server
import socketserver
import json
import sys
import threading

class MockProxyHandler(http.server.BaseHTTPRequestHandler):
    """Mock 代理处理器"""
    
    # 类变量用于记录请求
    request_log = []
    
    def log_request(self, code='-', size='-'):
        """记录请求"""
        MockProxyHandler.request_log.append({
            'method': self.command,
            'path': self.path,
            'code': code
        })
        super().log_request(code, size)
    
    def do_GET(self):
        """处理 GET 请求"""
        if '/test-success' in self.path:
            self.send_response(200)
            self.send_header('Content-type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'Mock proxy success')
        elif '/test-fail' in self.path:
            self.send_response(503)
            self.end_headers()
            self.wfile.write(b'Mock proxy failure')
        elif '/requests' in self.path:
            # 返回请求日志
            self.send_response(200)
            self.send_header('Content-type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(MockProxyHandler.request_log).encode())
        else:
            # 默认：转发请求（简化版）
            self.send_response(200)
            self.send_header('Content-type', 'text/plain')
            self.end_headers()
            self.wfile.write(b'Mock proxy response')
    
    def do_CONNECT(self):
        """处理 HTTPS CONNECT 请求"""
        self.send_response(200)
        self.end_headers()
    
    def do_POST(self):
        """处理 POST 请求"""
        self.send_response(200)
        self.send_header('Content-type', 'application/json')
        self.end_headers()
        self.wfile.write(b'{"status": "ok"}')

def run_proxy(port=8080):
    """启动代理服务器"""
    with socketserver.TCPServer(('', port), MockProxyHandler) as httpd:
        print(f'Mock proxy running on port {port}')
        httpd.serve_forever()

if __name__ == '__main__':
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    run_proxy(port)
