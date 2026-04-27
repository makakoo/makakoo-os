"""
AsyncExecutor — Non-blocking task runner for Harvey.

Phase 1 deliverable. Wraps long-running tool calls (image generation,
research, synthesis) in a ThreadPoolExecutor so the main Harvey event
loop never blocks. Results delivered via callback when complete.

Design:

  - Thin wrapper over concurrent.futures.ThreadPoolExecutor
  - Each submission gets a stable task_id so callers can poll or cancel
  - Callbacks run on the pool's thread (NOT the submitting thread) to
    avoid coupling the executor to the main event loop
  - No persistence: this is in-process only. Phase 1.5 will add a
    SQLite-backed artifact store for cross-process task results.
  - Thread-safe: internal state guarded by a single lock

Usage:

    executor = AsyncExecutor(max_workers=8)

    def on_done(task_id, result):
        print(f"{task_id} finished: {result}")

    def on_fail(task_id, exc):
        print(f"{task_id} failed: {exc}")

    tid = executor.submit(
        task_id="image_gen_1",
        fn=tool_generate_image,
        args=("a cat",),
        on_complete=on_done,
        on_error=on_fail,
    )

    # ... later
    executor.status(tid)   # "running" | "completed" | "failed"
    executor.wait(tid, timeout=30)
    executor.shutdown()
"""

from __future__ import annotations

import logging
import threading
import time
from concurrent.futures import Future, ThreadPoolExecutor, TimeoutError as FutureTimeout
from typing import Any, Callable, Dict, Optional, Tuple

log = logging.getLogger("harvey.async_executor")

DEFAULT_MAX_WORKERS = 8
DEFAULT_TASK_TIMEOUT_S = 300.0  # 5 min hard cap per task


class TaskInfo:
    __slots__ = (
        "task_id",
        "future",
        "submitted_at",
        "completed_at",
        "error",
        "result",
        "on_complete",
        "on_error",
    )

    def __init__(
        self,
        task_id: str,
        future: Future,
        on_complete: Optional[Callable[[str, Any], None]] = None,
        on_error: Optional[Callable[[str, Exception], None]] = None,
    ):
        self.task_id = task_id
        self.future = future
        self.submitted_at = time.time()
        self.completed_at: Optional[float] = None
        self.error: Optional[Exception] = None
        self.result: Any = None
        self.on_complete = on_complete
        self.on_error = on_error


class AsyncExecutor:
    """Non-blocking task executor with callback-based completion."""

    def __init__(self, max_workers: int = DEFAULT_MAX_WORKERS):
        self.max_workers = max_workers
        self._pool = ThreadPoolExecutor(
            max_workers=max_workers, thread_name_prefix="harvey-async"
        )
        self._tasks: Dict[str, TaskInfo] = {}
        self._lock = threading.Lock()
        self._shutdown = False

    # ─── Public API ──────────────────────────────────────────────

    def submit(
        self,
        task_id: str,
        fn: Callable[..., Any],
        args: Tuple = (),
        kwargs: Optional[Dict[str, Any]] = None,
        on_complete: Optional[Callable[[str, Any], None]] = None,
        on_error: Optional[Callable[[str, Exception], None]] = None,
    ) -> str:
        """
        Submit a task for background execution.

        Returns the task_id immediately. Task runs on the thread pool;
        on_complete/on_error callbacks fire from the pool's thread when
        the task finishes.
        """
        if self._shutdown:
            raise RuntimeError("AsyncExecutor is shut down")

        kwargs = kwargs or {}

        with self._lock:
            if task_id in self._tasks:
                existing = self._tasks[task_id]
                if not existing.future.done():
                    log.warning(
                        f"task_id {task_id} already running; returning existing"
                    )
                    return task_id
                # Otherwise stale entry, replace below

            future = self._pool.submit(self._run_wrapped, task_id, fn, args, kwargs)
            info = TaskInfo(
                task_id=task_id,
                future=future,
                on_complete=on_complete,
                on_error=on_error,
            )
            self._tasks[task_id] = info

        log.info(f"[async] submitted {task_id} (running={self.running_count()})")
        return task_id

    def status(self, task_id: str) -> str:
        """Return 'running' | 'completed' | 'failed' | 'unknown'."""
        with self._lock:
            info = self._tasks.get(task_id)
        if info is None:
            return "unknown"
        if not info.future.done():
            return "running"
        if info.error is not None:
            return "failed"
        return "completed"

    def wait(self, task_id: str, timeout: Optional[float] = None) -> Any:
        """Block until task completes. Raises the task's exception on failure."""
        with self._lock:
            info = self._tasks.get(task_id)
        if info is None:
            raise KeyError(f"Unknown task_id: {task_id}")
        try:
            return info.future.result(timeout=timeout)
        except FutureTimeout:
            raise TimeoutError(f"Task {task_id} did not complete within {timeout}s")

    def result(self, task_id: str) -> Any:
        """Return the task's result if completed, None otherwise."""
        with self._lock:
            info = self._tasks.get(task_id)
        if info is None or not info.future.done():
            return None
        return info.result

    def cancel(self, task_id: str) -> bool:
        """Attempt to cancel a task. Returns True if cancellation succeeded."""
        with self._lock:
            info = self._tasks.get(task_id)
        if info is None:
            return False
        return info.future.cancel()

    def running_count(self) -> int:
        """Number of tasks currently running (not done)."""
        with self._lock:
            return sum(1 for t in self._tasks.values() if not t.future.done())

    def task_count(self) -> int:
        """Total tasks tracked (running + finished)."""
        with self._lock:
            return len(self._tasks)

    def clear_finished(self) -> int:
        """Drop finished tasks from the tracking dict. Returns count removed."""
        with self._lock:
            finished = [tid for tid, t in self._tasks.items() if t.future.done()]
            for tid in finished:
                del self._tasks[tid]
        return len(finished)

    def shutdown(self, wait: bool = True) -> None:
        """Shut down the pool. wait=True blocks until in-flight tasks finish."""
        with self._lock:
            if self._shutdown:
                return
            self._shutdown = True
        log.info(f"[async] shutdown(wait={wait})")
        self._pool.shutdown(wait=wait)

    # ─── Internal ────────────────────────────────────────────────

    def _run_wrapped(
        self, task_id: str, fn: Callable, args: Tuple, kwargs: Dict
    ) -> Any:
        """
        Run the task function and fire callbacks on completion.

        Executes on a pool thread. Exceptions inside `fn` are captured
        and delivered via on_error. The result (or exception) is also
        stored on the TaskInfo for later polling via status()/result().
        """
        start = time.time()
        try:
            result = fn(*args, **kwargs)
        except Exception as e:
            log.exception(f"[async] task {task_id} raised")
            with self._lock:
                info = self._tasks.get(task_id)
                if info is not None:
                    info.error = e
                    info.completed_at = time.time()
            if info is not None and info.on_error is not None:
                try:
                    info.on_error(task_id, e)
                except Exception as cb_exc:
                    log.exception(f"[async] on_error callback crashed: {cb_exc}")
            raise

        duration = time.time() - start
        log.info(f"[async] task {task_id} done in {duration:.2f}s")

        with self._lock:
            info = self._tasks.get(task_id)
            if info is not None:
                info.result = result
                info.completed_at = time.time()
        if info is not None and info.on_complete is not None:
            try:
                info.on_complete(task_id, result)
            except Exception as cb_exc:
                log.exception(f"[async] on_complete callback crashed: {cb_exc}")

        return result


# ─────────────────────────────────────────────────────────────────────
# Module-level singleton for convenience (gateway uses this)
# ─────────────────────────────────────────────────────────────────────

_default_executor: Optional[AsyncExecutor] = None
_default_lock = threading.Lock()


def get_default_executor() -> AsyncExecutor:
    """Lazy-create the process-wide default executor."""
    global _default_executor
    with _default_lock:
        if _default_executor is None:
            _default_executor = AsyncExecutor()
    return _default_executor


def shutdown_default_executor() -> None:
    """Shut down the module-level singleton (call at process exit)."""
    global _default_executor
    with _default_lock:
        if _default_executor is not None:
            _default_executor.shutdown(wait=True)
            _default_executor = None


__all__ = [
    "AsyncExecutor",
    "TaskInfo",
    "get_default_executor",
    "shutdown_default_executor",
    "DEFAULT_MAX_WORKERS",
    "DEFAULT_TASK_TIMEOUT_S",
]
