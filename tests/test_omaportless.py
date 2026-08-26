import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MODULE_PATH = ROOT / "omaportless.py"
SPEC = importlib.util.spec_from_file_location("omaportless", MODULE_PATH)
assert SPEC and SPEC.loader
mod = importlib.util.module_from_spec(SPEC)
sys.modules["omaportless"] = mod
SPEC.loader.exec_module(mod)


class HostnameTests(unittest.TestCase):
    def test_slug(self):
        self.assertEqual(mod.slug("My App"), "my-app")
        self.assertEqual(mod.slug("  API__v2  "), "api-v2")

    def test_valid_hostname(self):
        self.assertTrue(mod.valid_hostname("dashboard"))
        self.assertTrue(mod.valid_hostname("api.myapp"))
        self.assertFalse(mod.valid_hostname("localhost"))
        self.assertTrue(mod.valid_hostname("Foo"))  # folded to lowercase
        self.assertFalse(mod.valid_hostname("-nope"))
        self.assertFalse(mod.valid_hostname(""))

    def test_short_name(self):
        self.assertEqual(mod.short_name("dashboard.localhost"), "dashboard")
        self.assertEqual(mod.short_name("dashboard.localhost."), "dashboard")
        self.assertEqual(mod.short_name("localhost"), "")
        self.assertEqual(mod.short_name("api.myapp.localhost"), "api.myapp")

    def test_index_host(self):
        self.assertTrue(mod.is_index_host("localhost"))
        self.assertTrue(mod.is_index_host("127.0.0.1"))
        self.assertTrue(mod.is_index_host("::1"))
        self.assertFalse(mod.is_index_host("dashboard.localhost"))

    def test_request_path(self):
        raw = b"GET /__omaportless/ping HTTP/1.1\r\nHost: x\r\n\r\n"
        self.assertEqual(mod.request_path(raw), "/__omaportless/ping")

    def test_hops(self):
        raw = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"
        bumped = mod.with_hop(raw, 1)
        self.assertEqual(mod.hop_count(bumped), 1)
        self.assertIn(b"X-Omaportless-Hops: 1", bumped)


class HostHeaderTests(unittest.TestCase):
    def test_parses_host(self):
        raw = b"GET / HTTP/1.1\r\nHost: dashboard.localhost:7777\r\n\r\n"
        self.assertEqual(mod.parse_host_header(raw), "dashboard.localhost")

    def test_ipv6_host(self):
        raw = b"GET / HTTP/1.1\r\nHost: [::1]:80\r\n\r\n"
        self.assertEqual(mod.parse_host_header(raw), "::1")


class RoutingTests(unittest.TestCase):
    def test_newest_wins(self):
        services = [
            {"hostname": "app", "port": 3000, "pid": 1, "starttime": 10, "alive": True},
            {"hostname": "app", "port": 3001, "pid": 2, "starttime": 20, "alive": True},
        ]
        picked = mod.newest_for_hostname(services, "app")
        self.assertEqual(picked["port"], 3001)

    def test_url_omits_port_80(self):
        self.assertEqual(mod.service_url("dash", 80), "http://dash.localhost")
        self.assertEqual(mod.service_url("dash", 7777), "http://dash.localhost:7777")

    def test_assign_names_suffixes_collisions(self):
        listeners = [
            {"port": 3000, "pid": 1, "cwd": "/tmp/app", "comm": "node", "cmd": "node", "starttime": 1},
            {"port": 3001, "pid": 2, "cwd": "/var/app", "comm": "node", "cmd": "node", "starttime": 2},
        ]
        named = mod.assign_names(listeners, {})
        hostnames = {item["hostname"] for item in named}
        self.assertEqual(len(hostnames), 2)


if __name__ == "__main__":
    unittest.main()
