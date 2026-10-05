#!/usr/bin/env python3
import json
import os
import socket
import sys


def config_path_from_args(args):
    try:
        return args[args.index("-f") + 1]
    except (ValueError, IndexError):
        return None


def socket_path_from_config(path):
    for line in open(path, encoding="utf-8"):
        key, separator, value = line.partition(":")
        if separator and key.strip() == "external-controller-unix":
            return value.strip().removeprefix("unix://")
    return None


def main():
    args = sys.argv[1:]
    config_path = config_path_from_args(args)
    with open("/var/lib/mihomo-cli/fake-core-argv", "w", encoding="utf-8") as output:
        output.write("\n".join(args))
        output.write("\n")

    if config_path is None:
        return 42

    socket_path = socket_path_from_config(config_path)
    if socket_path is None:
        return 43

    os.makedirs(os.path.dirname(socket_path), exist_ok=True)
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(socket_path)
    server.listen()
    try:
        while True:
            connection, _ = server.accept()
            with connection:
                request = connection.recv(65536)
                if request.startswith(b"PATCH /configs "):
                    response = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                else:
                    body = json.dumps({
                        "mode": "rule",
                        "mixed-port": 7890,
                        "port": 0,
                        "socks-port": 0,
                        "tun": {"enable": True, "stack": "system"},
                    }).encode()
                    response = (
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: "
                        + str(len(body)).encode()
                        + b"\r\n\r\n"
                        + body
                    )
                connection.sendall(response)
    finally:
        server.close()
        try:
            os.unlink(socket_path)
        except FileNotFoundError:
            pass


if __name__ == "__main__":
    raise SystemExit(main())
