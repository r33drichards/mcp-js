import io
import json
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

import run_code


class StartupRetryTest(unittest.TestCase):
    def test_retry_rebuilds_the_post_request_after_proxy_routing(self):
        url = "https://interpreter.example/api/exec"
        body = {"code": "console.log(15)", "session": "tutorial"}
        cold_start = HTTPError(
            url,
            503,
            "Service Unavailable",
            {},
            io.BytesIO(b'{"error":"no upstreams available"}'),
        )
        responses = iter((cold_start, io.BytesIO(b'{"execution_id":"accepted"}')))

        def open_via_proxy(request, *, timeout):
            self.assertEqual(request.type, "https")
            self.assertEqual(request.host, "interpreter.example")
            self.assertEqual(request.get_method(), "POST")
            self.assertEqual(request.get_header("Authorization"), "Bearer test-token")
            self.assertEqual(json.loads(request.data), body)
            request.set_proxy("proxy.example:8080", "http")
            response = next(responses)
            if isinstance(response, HTTPError):
                raise response
            return response

        with (
            patch.object(run_code, "urlopen", side_effect=open_via_proxy) as open_request,
            patch.object(run_code.time, "sleep") as sleep,
        ):
            result = run_code.request("https://interpreter.example", "test-token", "/api/exec", body)

        self.assertEqual(result, {"execution_id": "accepted"})
        self.assertEqual(open_request.call_count, 2)
        sleep.assert_called_once_with(run_code.STARTUP_RETRY_INTERVAL)


if __name__ == "__main__":
    unittest.main()
