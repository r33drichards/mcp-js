"""Synchronous, in-process JavaScript execution. No runtime subprocesses."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from threading import RLock
from typing import Any

__all__ = ["Engine", "EngineError", "ExecutionResult"]


class EngineError(RuntimeError):
    """Native initialization, lifecycle, or invocation failure."""


@dataclass(frozen=True)
class ExecutionResult:
    """Captured console output and optional JavaScript execution error."""

    output: str
    error: str | None = None
    artifacts: tuple[dict[str, Any], ...] = field(default_factory=tuple)

    @property
    def ok(self) -> bool:
        return self.error is None


class Engine:
    """A stateless native engine. Use a context manager or call close explicitly.

    JavaScript errors/timeouts are returned in ExecutionResult.error; invalid
    Python arguments raise TypeError/ValueError, native failures raise EngineError.
    Calls on one instance are serialized, including close.
    """

    def __init__(self, memory_mb: int = 64, timeout_secs: int = 2):
        for name, value, minimum, maximum in (
            ("memory_mb", memory_mb, 16, 4096),
            ("timeout_secs", timeout_secs, 1, 300),
        ):
            if type(value) is not int:
                raise TypeError(f"{name} must be an integer")
            if not minimum <= value <= maximum:
                raise ValueError(f"{name} must be between {minimum} and {maximum}")
        try:
            from . import _bindings
        except ImportError as error:
            raise EngineError(
                "Native bindings are missing; install a built mcp-js wheel or prepare the source package first"
            ) from error
        except OSError as error:
            raise EngineError(
                f"Cannot load the bundled native library: {error}"
            ) from error
        self._lock = RLock()
        self._bindings = _bindings
        try:
            self._native = _bindings.Engine.create_stateless(memory_mb, timeout_secs)
        except _bindings.RuntimeError as error:
            raise EngineError(str(error)) from error

    def run_js(self, code: str) -> ExecutionResult:
        if not isinstance(code, str):
            raise TypeError("code must be a string")
        with self._lock:
            if self._native is None:
                raise EngineError("engine is closed")
            try:
                result = json.loads(
                    self._native.call_tool(
                        "run_js",
                        json.dumps({"code": code}),
                        None,
                        None,
                    )
                )
            except self._bindings.RuntimeError as error:
                raise EngineError(str(error)) from error
            return ExecutionResult(
                output=result.get("output", ""),
                error=result.get("error"),
                artifacts=tuple(result.get("artifacts", ())),
            )

    def close(self) -> None:
        with self._lock:
            if self._native is not None:
                try:
                    self._native.close()
                except self._bindings.RuntimeError as error:
                    raise EngineError(str(error)) from error
                self._native = None

    def __enter__(self) -> Engine:  # noqa: PYI034 - Python 3.10 lacks typing.Self
        with self._lock:
            if self._native is None:
                raise EngineError("engine is closed")
        return self

    def __exit__(self, exc_type, exc_value, traceback) -> None:
        self.close()
