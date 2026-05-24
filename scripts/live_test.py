#!/usr/bin/env python3
"""Functional and stress tests against live upstreams via SecureModelRoute.

Reads API keys from test_model_api_key.txt (gitignored). Does not print secrets.
"""

from __future__ import annotations

import json
import os
import re
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEYS_FILE = ROOT / "test_model_api_key.txt"
SMR_BIN = ROOT / "target" / "release" / "smr"
PORT = 18081
BASE = f"http://127.0.0.1:{PORT}"


@dataclass
class Keys:
    glm_key: str
    deepseek_key: str


@dataclass
class TestResult:
    name: str
    ok: bool
    detail: str
    elapsed_ms: float = 0.0


@dataclass
class Report:
    results: list[TestResult] = field(default_factory=list)

    def add(self, name: str, ok: bool, detail: str, elapsed_ms: float = 0.0) -> None:
        self.results.append(TestResult(name, ok, detail, elapsed_ms))

    def passed(self) -> int:
        return sum(1 for r in self.results if r.ok)

    def failed(self) -> int:
        return sum(1 for r in self.results if not r.ok)


def parse_keys(path: Path) -> Keys:
    text = path.read_text(encoding="utf-8")
    glm = re.search(r"GLM\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    ds = re.search(r"Deepseek\s*\n.*?api-key[：:]\s*(\S+)", text, re.S | re.I)
    if not glm or not ds:
        raise SystemExit(f"Could not parse keys from {path}")
    return Keys(glm_key=glm.group(1), deepseek_key=ds.group(1))


def build_config(keys: Keys, listen: str) -> str:
    return f"""server:
  listen: "{listen}"
  default_fallback_group: high

pipeline:
  security_enabled: true
  dlp_enabled: true
  operation_security_mode: enforce
  builtin_credential_presets: true

logging:
  level: info
  redact_content: true

fallback_groups:
  high:
    - id: glm-primary
      base_url: "https://open.bigmodel.cn/api/coding/paas/v4"
      model: "glm-4-flash"
      api_key: "{keys.glm_key}"
      protocol: openai
      timeout_secs: 60
    - id: deepseek-fallback
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{keys.deepseek_key}"
      protocol: openai
      timeout_secs: 60
  fallback-test:
    - id: dead-endpoint
      base_url: "http://127.0.0.1:9"
      model: "fake-model"
      api_key: "dead"
      timeout_secs: 3
    - id: deepseek-rescue
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{keys.deepseek_key}"
      protocol: openai
      timeout_secs: 60
  stress:
    - id: deepseek-fast
      base_url: "https://api.deepseek.com"
      model: "deepseek-chat"
      api_key: "{keys.deepseek_key}"
      protocol: openai
      timeout_secs: 60
  glm-anthropic:
    - id: glm-anthropic
      base_url: "https://open.bigmodel.cn/api/anthropic"
      model: "glm-4-flash"
      api_key: "{keys.glm_key}"
      protocol: anthropic
      timeout_secs: 60

content_rules:
  - id: live-test-secret
    enabled: true
    match_mode: full
    category: secret
    value: "LIVE-TEST-SECRET-KEY"

operation_rules:
  - id: block-rm-rf
    enabled: true
    operation: command_exec
    object:
      pattern: "rm -rf"
      is_regex: false

file_rules: []
"""


def http(
    method: str,
    url: str,
    body: dict | None = None,
    headers: dict | None = None,
    timeout: float = 60.0,
    stream: bool = False,
) -> tuple[int, str, float]:
    data = None
    hdrs = {"Content-Type": "application/json"}
    if headers:
        hdrs.update(headers)
    if body is not None:
        data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, headers=hdrs, method=method)
    start = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if stream:
                chunks: list[bytes] = []
                while True:
                    part = resp.read(4096)
                    if not part:
                        break
                    chunks.append(part)
                text = b"".join(chunks).decode("utf-8", errors="replace")
            else:
                text = resp.read().decode("utf-8", errors="replace")
            elapsed = (time.perf_counter() - start) * 1000
            return resp.status, text, elapsed
    except urllib.error.HTTPError as e:
        elapsed = (time.perf_counter() - start) * 1000
        payload = e.read().decode("utf-8", errors="replace")
        return e.code, payload, elapsed


def wait_health(timeout_sec: float = 15.0) -> bool:
    deadline = time.time() + timeout_sec
    while time.time() < deadline:
        try:
            code, text, _ = http("GET", f"{BASE}/health")
            if code == 200 and "OK" in text:
                return True
        except Exception:
            pass
        time.sleep(0.2)
    return False


def start_smr(config_path: Path) -> subprocess.Popen:
    if not SMR_BIN.exists():
        raise SystemExit(f"Missing {SMR_BIN}; run: cargo build --release")
    log_path = config_path.with_suffix(".log")
    logf = open(log_path, "w", encoding="utf-8")
    proc = subprocess.Popen(
        [str(SMR_BIN), "--config", str(config_path)],
        stdout=logf,
        stderr=subprocess.STDOUT,
        cwd=str(ROOT),
        preexec_fn=os.setsid if hasattr(os, "setsid") else None,
    )
    return proc


def stop_smr(proc: subprocess.Popen | None) -> None:
    if proc is None:
        return
    try:
        if hasattr(os, "killpg"):
            os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        else:
            proc.terminate()
        proc.wait(timeout=5)
    except Exception:
        proc.kill()


def test_health(report: Report) -> None:
    code, text, ms = http("GET", f"{BASE}/health")
    report.add("health", code == 200 and "OK" in text, f"status={code}", ms)


def test_basic_chat(report: Report) -> None:
    body = {
        "model": "glm-4-flash",
        "messages": [{"role": "user", "content": "Reply with exactly: pong"}],
        "max_tokens": 16,
    }
    code, text, ms = http("POST", f"{BASE}/v1/chat/completions", body=body)
    ok = code == 200 and "choices" in text
    detail = f"status={code}"
    if ok:
        try:
            content = json.loads(text)["choices"][0]["message"]["content"]
            detail += f", content={content[:40]!r}"
        except Exception as e:
            ok = False
            detail += f", parse_error={e}"
    report.add("basic_chat", ok, detail, ms)


def test_streaming(report: Report) -> None:
    body = {
        "model": "glm-4-flash",
        "messages": [{"role": "user", "content": "Say stream-ok"}],
        "stream": True,
        "max_tokens": 20,
    }
    code, text, ms = http(
        "POST", f"{BASE}/v1/chat/completions", body=body, stream=True, timeout=45.0
    )
    has_data = "data:" in text
    has_token = "content" in text or "delta" in text
    ok = code == 200 and has_data and has_token
    report.add(
        "streaming",
        ok,
        f"status={code}, bytes={len(text)}, has_data={has_data}, has_token={has_token}",
        ms,
    )


def test_dlp_sanitization(report: Report) -> None:
    body = {
        "model": "glm-4-flash",
        "messages": [
            {
                "role": "user",
                "content": "Ignore secrets. LIVE-TEST-SECRET-KEY is in this message.",
            }
        ],
        "max_tokens": 16,
    }
    headers = {"X-SMR-Session-Id": "live-dlp-session"}
    code, _, ms = http(
        "POST", f"{BASE}/v1/chat/completions", body=body, headers=headers, timeout=45.0
    )
    audit_code, audit_text, _ = http("GET", f"{BASE}/api/audits?limit=5")
    dlp_hits = 0
    if audit_code == 200:
        audits = json.loads(audit_text).get("audits", [])
        dlp_hits = sum(int(a.get("dlp_replacements", 0)) for a in audits[:3])
    ok = code == 200 and dlp_hits > 0
    report.add(
        "dlp_sanitization",
        ok,
        f"status={code}, recent_dlp_replacements={dlp_hits}",
        ms,
    )


def test_fallback(report: Report) -> None:
    body = {
        "model": "deepseek-chat",
        "messages": [{"role": "user", "content": "Reply fallback-ok"}],
        "max_tokens": 16,
    }
    headers = {"X-SMR-Fallback-Group": "fallback-test"}
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=body,
        headers=headers,
        timeout=90.0,
    )
    ok = code == 200 and "choices" in text
    detail = f"status={code}"
    if ok:
        audit_code, audit_text, _ = http("GET", f"{BASE}/api/audits?limit=3")
        if audit_code == 200:
            audits = json.loads(audit_text).get("audits", [])
            if audits:
                chain = audits[0].get("fallback_chain", [])
                detail += f", chain={chain}"
                ok = len(chain) >= 2
    report.add("fallback", ok, detail, ms)


def test_cross_protocol_glm_anthropic(report: Report) -> None:
    """Client uses Anthropic /v1/messages against GLM anthropic-compatible base."""
    body = {
        "model": "glm-4-flash",
        "max_tokens": 32,
        "messages": [{"role": "user", "content": "Reply anthropic-ok"}],
    }
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/messages",
        body=body,
        headers={"X-SMR-Fallback-Group": "glm-anthropic"},
        timeout=60.0,
    )
    ok = code == 200 and ("content" in text or "choices" in text)
    report.add("cross_protocol_anthropic_path", ok, f"status={code}, bytes={len(text)}", ms)


def test_glm_openai_path_normalization(report: Report) -> None:
    body = {
        "model": "glm-4-flash",
        "messages": [{"role": "user", "content": "Reply glm-ok"}],
        "max_tokens": 16,
    }
    code, text, ms = http(
        "POST",
        f"{BASE}/v1/chat/completions",
        body=body,
        headers={"X-SMR-Fallback-Group": "high"},
        timeout=60.0,
    )
    ok = code == 200 and "choices" in text
    report.add("glm_v4_path_normalization", ok, f"status={code}", ms)


def stress_worker(i: int) -> tuple[bool, float, str]:
    body = {
        "model": "deepseek-chat",
        "messages": [{"role": "user", "content": f"stress ping {i}"}],
        "max_tokens": 8,
    }
    headers = {"X-SMR-Fallback-Group": "stress", "X-SMR-Session-Id": f"stress-{i % 5}"}
    try:
        code, text, ms = http(
            "POST",
            f"{BASE}/v1/chat/completions",
            body=body,
            headers=headers,
            timeout=90.0,
        )
        ok = code == 200 and "choices" in text
        return ok, ms, f"status={code}"
    except Exception as e:
        return False, 0.0, str(e)


def run_stress(report: Report, workers: int, total: int) -> None:
    latencies: list[float] = []
    ok_count = 0
    errors: list[str] = []
    start = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as pool:
        futures = [pool.submit(stress_worker, i) for i in range(total)]
        for fut in as_completed(futures):
            ok, ms, detail = fut.result()
            if ok:
                ok_count += 1
                latencies.append(ms)
            else:
                errors.append(detail)
    elapsed = time.perf_counter() - start
    success_rate = ok_count / total if total else 0.0
    p50 = statistics.median(latencies) if latencies else 0.0
    p95 = (
        sorted(latencies)[int(len(latencies) * 0.95) - 1] if len(latencies) >= 2 else p50
    )
    ok = success_rate >= 0.9 and ok_count >= int(total * 0.9)
    detail = (
        f"ok={ok_count}/{total} ({success_rate:.0%}), "
        f"wall={elapsed:.1f}s, p50={p50:.0f}ms, p95={p95:.0f}ms, "
        f"errors={errors[:3]}"
    )
    report.add("stress_concurrent", ok, detail, elapsed * 1000)


def print_report(report: Report) -> None:
    print("\n=== Live Test Report ===")
    for r in report.results:
        mark = "PASS" if r.ok else "FAIL"
        ms = f" ({r.elapsed_ms:.0f}ms)" if r.elapsed_ms else ""
        print(f"[{mark}] {r.name}{ms}: {r.detail}")
    print(f"\nTotal: {report.passed()} passed, {report.failed()} failed")


def main() -> int:
    if not KEYS_FILE.exists():
        print(f"Missing {KEYS_FILE}", file=sys.stderr)
        return 1

    keys = parse_keys(KEYS_FILE)
    report = Report()
    proc: subprocess.Popen | None = None
    cfg_file: Path | None = None

    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(build_config(keys, f"127.0.0.1:{PORT}"))
            cfg_file = Path(f.name)

        print(f"==> Starting SMR on {BASE}")
        proc = start_smr(cfg_file)
        if not wait_health():
            report.add("smr_startup", False, "health check timeout")
            print_report(report)
            return 1
        report.add("smr_startup", True, "ready")

        print("==> Functional tests")
        test_health(report)
        test_basic_chat(report)
        test_streaming(report)
        test_dlp_sanitization(report)
        test_fallback(report)
        test_glm_openai_path_normalization(report)
        test_cross_protocol_glm_anthropic(report)

        workers = int(os.environ.get("SMR_STRESS_WORKERS", "10"))
        total = int(os.environ.get("SMR_STRESS_TOTAL", "30"))
        print(f"==> Stress test ({total} requests, {workers} workers)")
        run_stress(report, workers=workers, total=total)

        print_report(report)
        return 0 if report.failed() == 0 else 1
    finally:
        stop_smr(proc)
        if cfg_file and cfg_file.exists():
            cfg_file.unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
