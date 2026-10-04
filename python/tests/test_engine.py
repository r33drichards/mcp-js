import subprocess
import unittest
from unittest.mock import patch


class EngineTests(unittest.TestCase):
    def test_execution_without_subprocesses(self):
        with patch.object(
            subprocess,
            "Popen",
            side_effect=AssertionError("runtime spawned a subprocess"),
        ):
            from mcp_js import Engine, EngineError

            with Engine(timeout_secs=1) as engine:
                result = engine.run_js("console.log(6 * 7)")
                self.assertTrue(result.ok)
                self.assertIn("42", result.output)
                self.assertIn(
                    "awaited",
                    engine.run_js(
                        'console.log(await Promise.resolve("awaited"))'
                    ).output,
                )
                self.assertIn("probe", engine.run_js('throw new Error("probe")').error)
                self.assertFalse(engine.run_js("while (true) {}").ok)
                self.assertIn(
                    "recovered", engine.run_js('console.log("recovered")').output
                )
                result = engine.run_js('artifact("test", "text/plain", "hello")')
                self.assertEqual(result.artifacts[0]["key"], "test")
            engine.close()
            with self.assertRaises(EngineError):
                engine.run_js("1")
            with self.assertRaises(EngineError), engine:
                pass

    def test_bad_arguments(self):
        from mcp_js import Engine

        for kwargs in ({"memory_mb": True}, {"timeout_secs": 1.5}):
            with self.assertRaises(TypeError):
                Engine(**kwargs)
        for kwargs in ({"memory_mb": 0}, {"timeout_secs": 301}):
            with self.assertRaises(ValueError):
                Engine(**kwargs)
        with Engine() as engine, self.assertRaises(TypeError):
            engine.run_js(None)

    def test_context_closes_after_exception(self):
        from mcp_js import Engine, EngineError

        with self.assertRaisesRegex(ValueError, "caller"), Engine() as engine:
            raise ValueError("caller")
        with self.assertRaises(EngineError):
            engine.run_js("1")


if __name__ == "__main__":
    unittest.main()
