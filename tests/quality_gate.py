from __future__ import annotations

import argparse
import concurrent.futures
import ctypes
import difflib
import gzip
import hashlib
import importlib.util
import os
import pickle
import re
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path


PHASE_LIMITS = {1: 0.058629, 2: 1.264281}
PHASE_SPEED_TOLERANCES = {1: 0.020000, 2: 0.010000}
RUST_TIMING_REPETITIONS = 1
DAD_REFERENCE_REPETITIONS = 1
DAD_CACHE_SCHEMA = 3
QUALITY_RUNTIME_ALLOWANCE_SECONDS = 0.100000
QUALITY_SMALL_GAIN_MAX_CLASSES = 10
QUALITY_REGRESSION_LIMIT_CLASSES = 50
DEFAULT_BALANCE_MS_PER_MISMATCH = 5.0
MAX_NORMALIZATION_REDUCTION_CLASSES = 5
# Hash of this benchmark source with this value replaced by "<fingerprint>".
# A mismatch marks the candidate as a comparator/normalization change even if
# the candidate was committed before measurement.
APPROVED_BENCHMARK_FINGERPRINT = "08e7c35e58ce89c71d4bd039d3bd8ba73e06ed5f819a5d38b58c0138d0fc1156"
ROOT = Path(__file__).resolve().parents[1]
RUST_CRATE = ROOT
LOCAL_NAME_RE = re.compile(r"\b(?:[vp]\d+(?:_\d+)?|vtmp\d+)\b")
LOCAL_DECLARATION_RE = re.compile(
    r"^(?P<indent>\s+)"
    r"(?P<type>[A-Za-z_$][\w$-]*(?:\.[A-Za-z_$][\w$-]*)*(?:\[\])?) "
    r"LOCAL;$"
)
INVALID_ARRAY_STORE_RE = re.compile(
    r"^(?P<indent>\s*)(?P<index>.+)\["
    r"(?P<type>[A-Za-z_$][\w$-]*(?:\.[A-Za-z_$][\w$-]*)*(?:\[\])?) "
    r"(?P<array>LOCAL)\] = (?P<value>.+);$"
)
UNSIGNED_LONG_DECLARATION_RE = re.compile(
    r"^(?P<prefix>\s+(?:(?:public|protected|private|static|final|volatile|transient|synthetic)\s+)*"
    r"long\s+[A-Za-z_$][\w$]*\s*=\s*)(?P<value>\d+)(?P<suffix>[lL]?;)$"
)
ENCODED_FLOAT_FIELD_RE = re.compile(
    r"^(?P<prefix> {4}(?:(?:public|protected|private|static|final|volatile|transient|synthetic)\s+)*"
    r"(?P<type>float|double)\s+[A-Za-z_$][\w$]*\s*=\s*)"
    r"(?P<value>[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)"
    r"(?P<suffix>[fFdD]?;)$"
)
INVALID_VOID_SUPER_ASSIGNMENT_RE = re.compile(
    r"^(?P<indent>\s*)void LOCAL = (?P<call>super\.[A-Za-z_$][\w$]*\(.*\);)$"
)
MULTI_CATCH_RE = re.compile(
    r"^(?P<prefix>\s*}\s*catch \()"
    r"(?P<types>[A-Za-z_$][\w$.-]*(?: \| [A-Za-z_$][\w$.-]*)+)"
    r"(?P<variable> LOCAL)?(?P<suffix>\) \{)$"
)
STRING_FIELD_RE = re.compile(
    r'^ {4}(?:(?:public|protected|private|static|final|volatile|transient|synthetic)\s+)*'
    r'String\s+[A-Za-z_$][\w$]*\s*=\s*".*";$'
)
PACKAGE_RE = re.compile(r"^package (?P<name>[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*);$")
CLASS_RE = re.compile(r"\b(?:class|interface|enum) (?P<name>[A-Za-z_$][\w$]*)\b")
FINAL_ARRAY_FIELD_RE = re.compile(
    r"^ {4}(?P<modifiers>(?:(?:public|protected|private|static|final|volatile|transient|synthetic)\s+)*)"
    r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*\[\]\s+(?P<name>[A-Za-z_$][\w$]*);$"
)
RAW_LOCAL = r"(?:[vp]\d+(?:_\d+)?|vtmp\d+)"
RAW_CATCH_RE = re.compile(
    rf"^(?P<prefix>\s*}}\s*catch \()"
    rf"(?P<types>[A-Za-z_$][\w$.-]*(?: \| [A-Za-z_$][\w$.-]*)*)"
    rf"(?P<variable> {RAW_LOCAL})?(?P<suffix>\) \{{)$"
)
RAW_BOOLEAN_ARRAY_NULL_RE = re.compile(
    rf"(?P<array>\b{RAW_LOCAL}\[(?:{RAW_LOCAL}|\d+)\]) "
    rf"(?P<operator>==|!=) (?:null|0)"
)
RAW_OBJECT_ARRAY_ZERO_RE = re.compile(
    rf"(?P<array>\b{RAW_LOCAL}\[(?:{RAW_LOCAL}|\d+)\]) "
    rf"(?P<operator>==|!=) 0"
)
RAW_PRIMITIVE_ARRAY_NULL_RE = re.compile(
    rf"(?P<array>\b{RAW_LOCAL}\[[^\[\]\n]+\]) "
    rf"(?P<operator>==|!=) null"
)
FINAL_ARRAY_ALIAS_RE = re.compile(
    rf"^\s*(?P<field>[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+)\s*=\s*(?P<local>{RAW_LOCAL});$"
)
STRING_LOCAL_DEFINITION_RE = re.compile(
    rf'^\s*[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*(?:\[\])* '
    rf'(?P<local>{RAW_LOCAL}) = (?P<literal>".*");$'
)
STRING_LITERAL_RE = re.compile(r'"(?:\\.|[^"\\])*"')
PURE_LITERAL_LOCAL_ASSIGNMENT_RE = re.compile(
    rf"^\s*(?:[A-Za-z_$][\w$-]*(?:\.[A-Za-z_$][\w$-]*)*(?:\[\])* )?"
    rf"(?P<local>{RAW_LOCAL}) = (?:{STRING_LITERAL_RE.pattern}|"
    r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?[fFdDlL]?|"
    r"true|false|null);$"
)
LOOP_HEADER_RE = re.compile(r"^\s*(?:while|for) \(|^\s*do \{$")
ARRAY_LENGTH_ASSIGNMENT_RE = re.compile(
    rf"^(?P<indent>\s*)(?:[A-Za-z_$][\w$.-]*(?:\[\])*\s+)?"
    rf"(?P<local>{RAW_LOCAL}) = (?P<array>{RAW_LOCAL})\.length;$"
)
UNUSED_INVOKE_RESULT_RE = re.compile(
    rf"^(?P<indent>\s*)[A-Za-z_$][\w$.-]*(?:\[\])* (?P<local>{RAW_LOCAL}) = "
    r"(?P<invoke>(?:this|super|[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+)"
    r"\.[A-Za-z_$][\w$]*\(.*\));$"
)
STABLE_LOCAL_ALIAS_RE = re.compile(
    rf"^(?P<indent>\s*)[A-Za-z_$][\w$.-]*(?:\[\])* "
    rf"(?P<local>{RAW_LOCAL}) = (?P<source>{RAW_LOCAL});$"
)
TERMINAL_LOCAL_MOVE_RE = re.compile(
    rf"^\s*(?:[A-Za-z_$][\w$.-]*(?:\[\])*\s+)?"
    rf"(?P<local>v(?:\d+(?:_\d+)?|tmp\d+)) = (?P<source>{RAW_LOCAL});$"
)
EMPTY_CATCH_LOCAL_UPDATE_RE = re.compile(
    rf"^\s*(?P<local>v(?:\d+(?:_\d+)?|tmp\d+))(?:\+\+|--);$"
)
DAD_INLINE_CONST_RE = re.compile(
    r"(?<![\w$.-])(?P<type>boolean|byte|short|char|int|long|float|double) "
    r"(?P<local>v\d+)(?![\w$])"
)
DAD_SPLIT_INLINE_CONST_RE = re.compile(
    r"(?<![\w$.-])(?P<type>boolean|byte|short|char|int|long|float|double|"
    r"[A-Za-z_$][\w$.-]*(?:\[\])*) (?P<local>v\d+(?:_\d+)?)(?![\w$])"
)
SPLIT_LOCAL_DECLARATION_RE = re.compile(
    r"(?:boolean|byte|short|char|int|long|float|double|"
    r"[A-Za-z_$][\w$.-]*(?:\[\])*) v\d+(?:_\d+)?\b(?:\s*=|\s*;)"
)
SPLIT_CONSTANT_DECLARATION_RE = re.compile(
    rf"^(?P<indent>\s*)(?P<type>boolean|byte|short|char|int|long|float|double|"
    rf"[A-Za-z_$][\w$.-]*(?:\[\])*) (?P<local>v\d+(?:_\d+)?) = "
    rf"(?P<literal>-?\d+|{STRING_LITERAL_RE.pattern});$"
)
LEXICAL_LOCAL_INITIALIZER_RE = re.compile(
    r"^(?P<indent>\s*)(?P<type>boolean|byte|short|char|int|long|float|double|String|"
    r"[A-Za-z_$][\w$.-]*(?:\[\])*) "
    r"(?P<local>v(?:\d+(?:_\d+)?|tmp\d+)) = (?P<rhs>.+);$"
)
LEXICAL_LOCAL_DECLARATION_RE = re.compile(
    r"^\s*(?:boolean|byte|short|char|int|long|float|double|String|"
    r"[A-Za-z_$][\w$.-]*(?:\[\])*) (?P<local>v(?:\d+(?:_\d+)?|tmp\d+))(?: = .+)?;$"
)
CANONICAL_LOCAL_DECLARATION_RE = re.compile(
    rf"^(?P<indent>\s+)(?P<type>[A-Za-z_$][\w$.-]*(?:\[\])*) "
    rf"(?P<local>{RAW_LOCAL})(?: = (?P<rhs>.+))?;$"
)
CATCH_LOCAL_RE = re.compile(rf"}}\s*catch\s*\([^)]*\b(?P<local>{RAW_LOCAL})\b")
THIS_LOCAL_ALIAS_RE = re.compile(
    rf"^(?P<indent>\s*)(?P<type>[A-Za-z_$][\w$.-]*) "
    rf"(?P<local>{RAW_LOCAL}) = this;$"
)
EMPTY_THIS_LOCAL_ALIAS_RE = re.compile(
    rf"^(?P<indent>\s*)(?P<type>[A-Za-z_$][\w$.-]*) "
    rf"(?P<local>{RAW_LOCAL}) = ;$"
)
TRANSITIVE_THIS_LOCAL_ALIAS_RE = re.compile(
    rf"^(?P<indent>\s*)(?P<type>[A-Za-z_$][\w$.-]*) "
    rf"(?P<local>{RAW_LOCAL}) = (?P<source>{RAW_LOCAL}|this);$"
)
UNINDENTED_CONSTRUCTOR_ARGUMENT_ARRAY_RE = re.compile(
    rf"^(?P<type>[A-Za-z_$][\w$.-]*) (?P<local>{RAW_LOCAL}) = new Object\[\d+\];$"
)
SUPER_CONSTRUCTOR_ALIAS_RE = re.compile(
    rf"^(?P<indent>\s*)(?P<type>[A-Za-z_$][\w$.-]*) "
    rf"(?P<local>{RAW_LOCAL}) = (?P<call>super\(.*\);)$"
)
INLINE_LOCAL_DECLARATION_RE = re.compile(
    r"(?<![\w$.-])(?:boolean|byte|short|char|int|long|float|double|String|"
    r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+) (?P<local>LOCAL)"
)
LEADING_LOCAL_DECLARATION_RE = re.compile(
    r"^\s*(?:boolean|byte|short|char|int|long|float|double|String|"
    r"[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+) LOCAL(?:\s*[=;])"
)
IF_STATEMENT_RE = re.compile(r"^(?P<indent>\s*)if \((?P<condition>.*)\) \{$")
SWITCH_CASE_LABEL_RE = re.compile(r"^(?P<indent>\s*)case .+:$")


def evaluate_optimization_policy(
    mode: str,
    *,
    mismatches: int,
    rust_seconds: float,
    baseline_mismatches: int,
    baseline_rust_seconds: float,
    baseline_balance_score_ms: float | None = None,
    balance_ms_per_mismatch: float = DEFAULT_BALANCE_MS_PER_MISMATCH,
    normalization_changed: bool = False,
    normalization_control_mismatches: int | None = None,
) -> tuple[bool, str, float]:
    quality_delta = baseline_mismatches - mismatches
    speed_improved = rust_seconds < baseline_rust_seconds
    within_quality_budget = mismatches <= baseline_mismatches + QUALITY_REGRESSION_LIMIT_CLASSES
    balance_score_ms = rust_seconds * 1000.0 + balance_ms_per_mismatch * mismatches
    if normalization_changed:
        if normalization_control_mismatches is None:
            return (
                False,
                "normalization_control_missing=true "
                "action=measure_same_rust_with_approved_comparator_first",
                balance_score_ms,
            )
        normalization_reduction = normalization_control_mismatches - mismatches
        if normalization_reduction > MAX_NORMALIZATION_REDUCTION_CLASSES:
            return (
                False,
                "normalization_overreach=true "
                f"normalization_reduction={normalization_reduction} "
                f"limit={MAX_NORMALIZATION_REDUCTION_CLASSES} "
                "action=rollback_normalization_reflect_and_fix_rust",
                balance_score_ms,
            )
    if mode == "performance":
        accepted = within_quality_budget and speed_improved
        reason = f"quality_budget={within_quality_budget} speed_improved={speed_improved}"
    elif mode == "quality":
        if quality_delta > QUALITY_SMALL_GAIN_MAX_CLASSES:
            runtime_ok = rust_seconds <= baseline_rust_seconds + QUALITY_RUNTIME_ALLOWANCE_SECONDS
            accepted = runtime_ok
            reason = f"significant_quality_gain={quality_delta} runtime_ok={runtime_ok}"
        elif quality_delta > 0:
            runtime_ok = rust_seconds <= baseline_rust_seconds
            accepted = runtime_ok
            reason = f"small_quality_gain={quality_delta} runtime_nonregression={runtime_ok}"
        else:
            accepted = within_quality_budget and speed_improved
            reason = f"quality_gain={quality_delta} quality_budget={within_quality_budget} speed_improved={speed_improved}"
    elif mode == "balanced":
        if baseline_balance_score_ms is None:
            raise ValueError("balanced mode requires a balance baseline")
        runtime_ok = rust_seconds <= baseline_rust_seconds + QUALITY_RUNTIME_ALLOWANCE_SECONDS
        score_improved = balance_score_ms < baseline_balance_score_ms
        accepted = within_quality_budget and runtime_ok and score_improved
        reason = f"quality_budget={within_quality_budget} runtime_ok={runtime_ok} score_improved={score_improved}"
    else:
        raise ValueError(f"unknown optimization mode: {mode}")
    return accepted, reason, balance_score_ms


def validate_optimization_policies() -> None:
    baseline_mismatches = 307
    baseline_seconds = 1.28972372
    baseline_score = 2824.72372
    assert evaluate_optimization_policy("performance", mismatches=357, rust_seconds=1.280, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds)[0]
    assert not evaluate_optimization_policy("performance", mismatches=358, rust_seconds=1.200, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds)[0]
    assert not evaluate_optimization_policy("quality", mismatches=306, rust_seconds=1.34658090, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds)[0]
    assert evaluate_optimization_policy("quality", mismatches=296, rust_seconds=baseline_seconds + QUALITY_RUNTIME_ALLOWANCE_SECONDS, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds)[0]
    rejected = evaluate_optimization_policy("balanced", mismatches=306, rust_seconds=1.34658090, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds, baseline_balance_score_ms=baseline_score)
    assert not rejected[0] and rejected[2] > baseline_score
    accepted = evaluate_optimization_policy("balanced", mismatches=306, rust_seconds=1.284, baseline_mismatches=baseline_mismatches, baseline_rust_seconds=baseline_seconds, baseline_balance_score_ms=baseline_score)
    assert accepted[0] and accepted[2] < baseline_score
    normalization_without_control = evaluate_optimization_policy(
        "quality", mismatches=302, rust_seconds=baseline_seconds,
        baseline_mismatches=baseline_mismatches,
        baseline_rust_seconds=baseline_seconds, normalization_changed=True,
    )
    assert not normalization_without_control[0]
    assert "normalization_control_missing=true" in normalization_without_control[1]
    normalization_limit = evaluate_optimization_policy(
        "quality", mismatches=302, rust_seconds=baseline_seconds,
        baseline_mismatches=baseline_mismatches,
        baseline_rust_seconds=baseline_seconds, normalization_changed=True,
        normalization_control_mismatches=307,
    )
    assert normalization_limit[0]
    normalization_overreach = evaluate_optimization_policy(
        "quality", mismatches=301, rust_seconds=baseline_seconds,
        baseline_mismatches=baseline_mismatches,
        baseline_rust_seconds=baseline_seconds, normalization_changed=True,
        normalization_control_mismatches=307,
    )
    assert not normalization_overreach[0]
    assert "normalization_overreach=true" in normalization_overreach[1]
    assert classify_candidate_scope({"src/writer.rs"}, False) == (True, False)
    assert classify_candidate_scope(
        {"test/tools/benchmark_diff_dad_rad.py"}, True
    ) == (False, True)
    assert classify_candidate_scope(
        {"src/writer.rs", "test/tools/benchmark_diff_dad_rad.py"}, True
    ) == (True, True)


def benchmark_fingerprint() -> str:
    source = Path(__file__).read_text(encoding="utf-8").replace("\r\n", "\n")
    canonical = re.sub(
        r'(?m)^APPROVED_BENCHMARK_FINGERPRINT = "[0-9a-f]{64}"$',
        'APPROVED_BENCHMARK_FINGERPRINT = "<fingerprint>"',
        source,
    )
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def changed_worktree_paths() -> set[str]:
    """Return tracked and untracked candidate paths relative to the repo root."""
    try:
        tracked = subprocess.run(
            ["git", "diff", "--name-only", "--diff-filter=ACMRTUXB", "HEAD"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        untracked = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
    except (OSError, subprocess.CalledProcessError):
        return set()
    return {path.replace("\\", "/") for path in tracked + untracked}


def classify_candidate_scope(
    paths: set[str], benchmark_changed: bool
) -> tuple[bool, bool]:
    """Classify changes capable of altering output or mismatch counting.

    A normalization change includes comparator/oracle code, the DAD reference
    implementation/cache, and benchmark DEX inputs. Benchmark-source changes
    are conservatively treated as normalization changes because this guard
    cannot prove that an edit is policy-only.
    """
    rust_production_changed = any(
        path.startswith("src/") or path in {"Cargo.toml", "Cargo.lock"}
        for path in paths
    )
    comparison_input_changed = any(
        path == "test/tools/benchmark_diff_dad_rad.py"
        or path.startswith("dadguard_py/")
        or path in {"tests/fixtures/sample.dex", "tests/fixtures/classes5.dex"}
        or path.startswith("tests/fixtures/dad_reference_cache/")
        for path in paths
    )
    return rust_production_changed, benchmark_changed or comparison_input_changed


def _load_local_dadguard() -> None:
    raise RuntimeError(
        "Androguard/DAD execution is disabled: all reference output and comparison oracles "
        "must come from the checked-in frozen pickle cache"
    )


class _AnalysisCache:
    def __init__(self, vm, method_analysis_cls):
        self.vm = vm
        self.method_analysis_cls = method_analysis_cls
        self.methods = {}
        self.classes = {}

    def get_method(self, method):
        if method not in self.methods:
            self.methods[method] = self.method_analysis_cls(self.vm, method)
        return self.methods[method]


def decompile_dad(dex_bytes: bytes) -> tuple[dict[str, str], float]:
    raise RuntimeError(
        "DAD generation is disabled: the quality gate requires the checked-in frozen pickle cache"
    )


def dad_cache_stem(dex_label: str) -> str:
    stem = Path(dex_label).stem
    safe = re.sub(r"[^A-Za-z0-9._-]+", "_", stem).strip("._-")
    return safe or "dex"


def cached_dad_output(payload: object) -> tuple[dict[str, str] | None, bool]:
    if not isinstance(payload, dict):
        return None, False
    output = payload.get("output")
    if isinstance(output, dict):
        return output, False
    # Migrate the preceding one-reference format without running dadguard.
    outputs = payload.get("outputs")
    if (
        isinstance(outputs, list)
        and len(outputs) == 1
        and isinstance(outputs[0], dict)
    ):
        return outputs[0], True
    return None, False


def write_dad_cache(
    cache_path: Path,
    dex_label: str,
    output: dict[str, str],
    generation_seconds: float,
) -> None:
    payload = {
        "schema": DAD_CACHE_SCHEMA,
        "dex": Path(dex_label).name,
        "generation_seconds": generation_seconds,
        "output": output,
    }
    temporary = cache_path.with_name(f".{cache_path.name}.{os.getpid()}.tmp")
    try:
        with gzip.open(temporary, "wb", compresslevel=1) as stream:
            pickle.dump(payload, stream, protocol=pickle.HIGHEST_PROTOCOL)
        os.replace(temporary, cache_path)
    finally:
        temporary.unlink(missing_ok=True)


def decompile_dad_cached(
    dex_bytes: bytes,
    cache_dir: Path | None,
    refresh_cache: bool,
    dex_label: str,
) -> tuple[dict[str, str], float]:
    if cache_dir is None:
        return decompile_dad(dex_bytes)

    started = time.perf_counter()
    cache_dir.mkdir(parents=True, exist_ok=True)
    cache_path = cache_dir / f"dad-{dad_cache_stem(dex_label)}.pickle.gz"
    if not refresh_cache and cache_path.is_file():
        try:
            with gzip.open(cache_path, "rb") as stream:
                payload = pickle.load(stream)
        except (EOFError, OSError, pickle.PickleError, AttributeError, TypeError) as error:
            raise RuntimeError(
                f"DAD cache is unreadable: {cache_path}; rerun with "
                "--refresh-dad-cache"
            ) from error
        output, _ = cached_dad_output(payload)
        if output is None:
            raise RuntimeError(f"DAD cache has an unsupported payload: {cache_path}")
        if payload.get("oracle_schema") != 1 or not isinstance(payload.get("oracles"), dict):
            raise RuntimeError(f"DAD cache lacks frozen comparison oracles: {cache_path}")
        print(
            f"DAD_CACHE hit path={cache_path} classes={len(output)}",
            flush=True,
        )
        return output, payload["oracles"], time.perf_counter() - started

    raise RuntimeError(
        f"required frozen DAD cache is missing: {cache_path}; cache regeneration is disabled"
    )


def clear_diff_directory(directory: Path) -> None:
    resolved = directory.resolve()
    test_root = (ROOT / "test").resolve()
    if not resolved.is_relative_to(test_root) or resolved == test_root:
        raise RuntimeError(f"refusing to clear diff directory outside {test_root}: {resolved}")
    if resolved.exists():
        if not resolved.is_dir():
            raise RuntimeError(f"diff output path is not a directory: {resolved}")
        shutil.rmtree(resolved)
    resolved.mkdir(parents=True)
    print(f"DIFF_DIR cleared path={resolved}", flush=True)


def build_rust() -> Path:
    subprocess.run(
        ["cargo", "build", "--release"],
        cwd=RUST_CRATE,
        check=True,
    )
    executable = RUST_CRATE / "target" / "release" / (
        "radguard.exe" if os.name == "nt" else "radguard"
    )
    if not executable.is_file():
        raise RuntimeError(f"Rust executable was not produced: {executable}")
    return executable


def pin_current_process(cpu: int) -> None:
    if cpu < 0:
        print("AFFINITY disabled", flush=True)
        return
    if os.name == "nt":
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel32.GetCurrentProcess.restype = ctypes.c_void_p
        kernel32.GetProcessAffinityMask.argtypes = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_size_t),
        ]
        kernel32.GetProcessAffinityMask.restype = ctypes.c_int
        kernel32.SetProcessAffinityMask.argtypes = [
            ctypes.c_void_p,
            ctypes.c_size_t,
        ]
        kernel32.SetProcessAffinityMask.restype = ctypes.c_int
        process_mask = ctypes.c_size_t()
        system_mask = ctypes.c_size_t()
        handle = kernel32.GetCurrentProcess()
        if kernel32.GetProcessAffinityMask(
            handle,
            ctypes.byref(process_mask),
            ctypes.byref(system_mask),
        ) == 0:
            raise OSError(
                f"GetProcessAffinityMask failed: {ctypes.get_last_error()}"
            )
        if cpu == 0:
            cpu = (process_mask.value & -process_mask.value).bit_length() - 1
        mask = 1 << cpu
        if process_mask.value & mask == 0:
            raise OSError(
                f"CPU {cpu} is outside current affinity mask 0x{process_mask.value:x}"
            )
        result = kernel32.SetProcessAffinityMask(
            handle,
            ctypes.c_size_t(mask),
        )
        if result == 0:
            raise OSError(
                f"SetProcessAffinityMask failed for CPU {cpu}: "
                f"{ctypes.get_last_error()}"
            )
    elif hasattr(os, "sched_setaffinity"):
        os.sched_setaffinity(0, {cpu})
    else:
        raise RuntimeError("single-CPU affinity is unsupported on this platform")
    print(f"AFFINITY cpu={cpu}", flush=True)


def decompile_rust(
    executable: Path,
    dex_path: Path,
    class_name: str | None = None,
    threads: int = 1,
) -> tuple[dict[str, str], float]:
    if class_name is not None:
        started = time.perf_counter()
        result = subprocess.run(
            [
                str(executable),
                "decompile",
                "--no-metadata",
                str(dex_path),
                class_name,
            ],
            check=True,
            capture_output=True,
        )
        return {class_name: result.stdout.decode("utf-8")}, time.perf_counter() - started
    command = (
        [str(executable), "export-all", "--no-metadata", str(dex_path)]
        if threads == 1
        else [
            str(executable),
            "export-all-parallel",
            "--no-metadata",
            str(dex_path),
            str(threads),
        ]
    )
    result = subprocess.run(
        command,
        check=True,
        capture_output=True,
    )
    data = memoryview(result.stdout)
    if len(data) < 20 or bytes(data[:8]) != b"RGOUT001":
        raise RuntimeError(
            f"invalid radguard export stream: {result.stderr.decode(errors='replace')}"
        )
    seconds = struct.unpack_from("<d", data, 8)[0]
    class_count = struct.unpack_from("<I", data, 16)[0]
    offset = 20
    output = {}
    for _ in range(class_count):
        if offset + 12 > len(data):
            raise RuntimeError("truncated radguard class record")
        descriptor_len, source_len = struct.unpack_from("<IQ", data, offset)
        offset += 12
        end_descriptor = offset + descriptor_len
        end_source = end_descriptor + source_len
        if end_source > len(data):
            raise RuntimeError("truncated radguard class payload")
        descriptor = bytes(data[offset:end_descriptor]).decode("utf-8")
        source = bytes(data[end_descriptor:end_source]).decode("utf-8")
        if descriptor in output:
            raise RuntimeError(f"duplicate radguard class record: {descriptor}")
        output[descriptor] = source
        offset = end_source
    if offset != len(data):
        raise RuntimeError("trailing bytes in radguard export stream")
    return output, seconds


def safe_class_name(descriptor: str) -> str:
    return (
        descriptor.replace("/", "_")
        .replace("\\", "_")
        .replace(";", "")
        .replace("$", "_")
    )


def normalize_invalid_array_store(line: str) -> str:
    match = INVALID_ARRAY_STORE_RE.match(line)
    if match is None:
        return line
    return (
        f"{match['indent']}{match['array']}[{match['index']}] = "
        f"{match['value']};"
    )


def dad_type_name_corrections(dex_bytes: bytes) -> dict[str, str]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module
    from dadguard.decompiler.util import get_type as dad_get_type

    vm = dex_module.DEX(dex_bytes)
    candidates: dict[str, set[str]] = {}
    for index in range(vm.header.type_ids_size):
        descriptor = vm.CM.get_type(index)
        if not (
            isinstance(descriptor, str)
            and descriptor.startswith("Ljava/lang/")
            and descriptor.endswith(";")
        ):
            continue
        correct = descriptor[1:-1].replace("/", ".").removeprefix("java.lang.")
        buggy = dad_get_type(descriptor)
        if buggy != correct:
            candidates.setdefault(buggy, set()).add(correct)
    return {
        buggy: next(iter(correct))
        for buggy, correct in candidates.items()
        if len(correct) == 1
    }


def normalize_dad_type_names(source: str, corrections: dict[str, str]) -> str:
    if not corrections:
        return source
    pattern = re.compile(
        r"(?<![\w$.-])(?:"
        + "|".join(re.escape(name) for name in sorted(corrections, key=len, reverse=True))
        + r")(?![\w$.-])"
    )
    lines = []
    for line in source.splitlines():
        parts = re.split(r'("(?:\\.|[^"\\])*")', line)
        for index in range(0, len(parts), 2):
            parts[index] = pattern.sub(lambda match: corrections[match.group(0)], parts[index])
        lines.append("".join(parts))
    return "\n".join(lines)


def descriptor_to_java(descriptor: str) -> str:
    if descriptor.startswith("["):
        return f"{descriptor_to_java(descriptor[1:])}[]"
    primitive = {
        "V": "void",
        "Z": "boolean",
        "B": "byte",
        "S": "short",
        "C": "char",
        "I": "int",
        "J": "long",
        "F": "float",
        "D": "double",
    }.get(descriptor)
    if primitive is not None:
        return primitive
    if descriptor.startswith("L") and descriptor.endswith(";"):
        name = descriptor[1:-1].replace("/", ".")
        return name.removeprefix("java.lang.")
    return descriptor


def boolean_array_load_oracle(dex_bytes: bytes) -> dict[str, list[int]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module

    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            code = method.get_code()
            if code is None:
                continue
            methods.append(
                sum(
                    instruction.get_op_value() == 0x47
                    for instruction in code.get_bc().get_instructions()
                )
            )
        output[class_def.get_name()] = methods
    return output


def normalize_dad_boolean_array_nulls(source: str, oracle: list[int]) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), expected_count in zip(sections, oracle):
        hits = sum(
            len(RAW_BOOLEAN_ARRAY_NULL_RE.findall(lines[index]))
            for index in range(start, end + 1)
        )
        if hits == 0 or hits > expected_count:
            continue
        for index in range(start, end + 1):
            lines[index] = RAW_BOOLEAN_ARRAY_NULL_RE.sub(
                lambda match: (
                    f'!{match["array"]}'
                    if match["operator"] == "=="
                    else match["array"]
                ),
                lines[index],
            )
    return "\n".join(lines)


def object_array_load_oracle(dex_bytes: bytes) -> dict[str, list[int]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module

    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            code = method.get_code()
            if code is None:
                continue
            methods.append(
                sum(
                    instruction.get_op_value() == 0x46
                    for instruction in code.get_bc().get_instructions()
                )
            )
        output[class_def.get_name()] = methods
    return output


def normalize_dad_object_array_zeroes(source: str, oracle: list[int]) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), expected_count in zip(sections, oracle):
        hits = sum(
            len(RAW_OBJECT_ARRAY_ZERO_RE.findall(lines[index]))
            for index in range(start, end + 1)
        )
        if hits == 0 or hits > expected_count:
            continue
        for index in range(start, end + 1):
            lines[index] = RAW_OBJECT_ARRAY_ZERO_RE.sub(
                lambda match: f'{match["array"]} {match["operator"]} null',
                lines[index],
            )
    return "\n".join(lines)


def primitive_array_load_oracle(dex_bytes: bytes) -> dict[str, list[int]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module

    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            code = method.get_code()
            if code is None:
                continue
            methods.append(
                sum(
                    instruction.get_op_value() in (0x44, 0x45, 0x48, 0x49, 0x4A)
                    for instruction in code.get_bc().get_instructions()
                )
            )
        output[class_def.get_name()] = methods
    return output


def normalize_dad_primitive_array_nulls(source: str, oracle: list[int]) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), expected_count in zip(sections, oracle):
        hits = sum(
            len(RAW_PRIMITIVE_ARRAY_NULL_RE.findall(lines[index]))
            for index in range(start, end + 1)
        )
        if hits == 0 or hits > expected_count:
            continue
        for index in range(start, end + 1):
            lines[index] = RAW_PRIMITIVE_ARRAY_NULL_RE.sub(
                lambda match: f'{match["array"]} {match["operator"]} 0',
                lines[index],
            )
    return "\n".join(lines)


def inline_const_oracle(dex_bytes: bytes) -> dict[str, list[dict[str, str]]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module

    write_opcodes = (
        set(range(0x01, 0x0E))
        | set(range(0x12, 0x1D))
        | set(range(0x1F, 0x24))
        | set(range(0x2D, 0x32))
        | set(range(0x44, 0x4B))
        | set(range(0x52, 0x59))
        | set(range(0x60, 0x67))
        | set(range(0x7B, 0xE3))
    )
    wide_destinations = {
        0x04,
        0x05,
        0x06,
        0x0B,
        *range(0x16, 0x1A),
        0x45,
        0x53,
        0x61,
        0x7D,
        0x7E,
        0x80,
        0x81,
        0x83,
        0x86,
        0x88,
        0x89,
        0x8B,
        *range(0x9B, 0xA6),
        *range(0xAB, 0xB0),
        *range(0xBB, 0xC6),
        *range(0xCB, 0xD0),
    }
    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            code = method.get_code()
            if code is None:
                continue
            definitions: dict[int, list[tuple[int, int | None]]] = {}
            for instruction in code.get_bc().get_instructions():
                opcode = instruction.get_op_value()
                if opcode not in write_opcodes:
                    continue
                register = getattr(instruction, "A", getattr(instruction, "AA", None))
                if register is None:
                    continue
                definitions.setdefault(register, []).append(
                    (opcode, getattr(instruction, "B", None))
                )
                if opcode in wide_destinations:
                    definitions.setdefault(register + 1, []).append((opcode, None))
            methods.append(
                {
                    f"v{register}": str(definition[0][1])
                    for register, definition in definitions.items()
                    if len(definition) == 1
                    and definition[0][0] == 0x12
                    and definition[0][1] is not None
                }
            )
        output[class_def.get_name()] = methods
    return output


def split_inline_const_oracle(dex_bytes: bytes) -> dict[str, list[dict[str, str]]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module
    from dadguard.core.analysis.analysis import MethodAnalysis
    from dadguard.decompiler.dataflow import build_def_use, split_variables
    from dadguard.decompiler.decompile import DvMethod
    from dadguard.decompiler.graph import construct
    from dadguard.decompiler.instruction import Constant

    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            if method.get_code() is None:
                continue
            try:
                decompiled = DvMethod(MethodAnalysis(vm, method))
                graph = construct(
                    decompiled.start_block,
                    decompiled.var_to_name,
                    decompiled.exceptions,
                )
                use_defs, def_uses = build_def_use(graph, decompiled.lparams)
                split_variables(graph, decompiled.var_to_name, def_uses, use_defs)
                definitions: dict[int, list[int]] = {}
                for (variable, location), _ in def_uses.items():
                    definitions.setdefault(variable, []).append(location)
                literals = {}
                for variable, locations in definitions.items():
                    if len(locations) != 1:
                        continue
                    instruction = graph.get_ins_from_loc(locations[0])
                    rhs = instruction.get_rhs() if instruction is not None else None
                    name = getattr(decompiled.var_to_name.get(variable), "name", None)
                    value = rhs.cst if isinstance(rhs, Constant) else None
                    if isinstance(name, (str, int)) and isinstance(value, (int, str)):
                        literals[f"v{name}"] = value
                methods.append(literals)
            except Exception:
                methods.append({})
        output[class_def.get_name()] = methods
    return output


def catch_type_oracle(dex_bytes: bytes) -> dict[str, list[list[list[str]]]]:
    _load_local_dadguard()
    from dadguard.core.analysis.analysis import MethodAnalysis
    from dadguard.core import dex as dex_module
    from dadguard.decompiler.graph import bfs

    vm = dex_module.DEX(dex_bytes)
    output = {}
    for class_def in vm.get_classes():
        methods = []
        for method in class_def.get_methods():
            if method.get_code() is None:
                continue
            analysis = MethodAnalysis(vm, method)
            start = next(analysis.get_basic_blocks().get(), None)
            targets: dict[int, list[str]] = {}
            if start is not None:
                for block in bfs(start):
                    exception = block.exception_analysis
                    if exception is None:
                        continue
                    for descriptor, _, target in exception.exceptions:
                        types = targets.setdefault(target.start, [])
                        java_type = descriptor_to_java(descriptor)
                        if java_type not in types:
                            types.append(java_type)
            methods.append(list(targets.values()))
        output[class_def.get_name()] = methods
    return output


def method_sections(lines: list[str]) -> list[tuple[int, int]]:
    sections = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            return []
        sections.append((index, end))
        index = end + 1
    return sections


def normalize_dad_catch_types(
    dad_source: str, rust_source: str, oracle: list[list[list[str]]]
) -> tuple[str, str]:
    dad_lines = dad_source.splitlines()
    rust_lines = rust_source.splitlines()
    dad_methods = method_sections(dad_lines)
    rust_methods = method_sections(rust_lines)
    if len(dad_methods) != len(oracle) or len(rust_methods) != len(oracle):
        return dad_source, rust_source

    for method_index, (dad_section, rust_section, expected) in enumerate(
        zip(dad_methods, rust_methods, oracle)
    ):
        dad_catches = [
            (index, match)
            for index in range(dad_section[0], dad_section[1] + 1)
            if (match := RAW_CATCH_RE.match(dad_lines[index])) is not None
        ]
        rust_catches = [
            (index, match)
            for index in range(rust_section[0], rust_section[1] + 1)
            if (match := RAW_CATCH_RE.match(rust_lines[index])) is not None
        ]
        if not rust_catches or len(dad_catches) != len(rust_catches):
            continue
        expected_types = [types for types in expected]
        rust_types = [match["types"].split(" | ") for _, match in rust_catches]
        dad_types = [match["types"].split(" | ") for _, match in dad_catches]
        exact_rust_sequence = rust_types == expected_types
        exact_single_dad_target = (
            len(expected_types) == 1
            and len(rust_types) == 1
            and dad_types == expected_types
        )
        exact_homogeneous_dad_targets = (
            bool(expected_types)
            and all(types == expected_types[0] for types in expected_types)
            and dad_types == expected_types
        )
        matched_catches = set()
        if exact_rust_sequence or exact_single_dad_target or exact_homogeneous_dad_targets:
            matched_catches.update(range(len(rust_catches)))
        else:
            prefix = [[0] * (len(expected_types) + 1) for _ in range(len(rust_types) + 1)]
            for left in range(len(rust_types)):
                for right in range(len(expected_types)):
                    prefix[left + 1][right + 1] = prefix[left][right] + 1 if rust_types[left] == expected_types[right] else max(prefix[left][right + 1], prefix[left + 1][right])
            suffix = [[0] * (len(expected_types) + 1) for _ in range(len(rust_types) + 1)]
            for left in range(len(rust_types) - 1, -1, -1):
                for right in range(len(expected_types) - 1, -1, -1):
                    suffix[left][right] = suffix[left + 1][right + 1] + 1 if rust_types[left] == expected_types[right] else max(suffix[left + 1][right], suffix[left][right + 1])
            best = prefix[-1][-1]
            for left in range(len(rust_types)):
                positions = [right for right in range(len(expected_types)) if rust_types[left] == expected_types[right] and prefix[left][right] + 1 + suffix[left + 1][right + 1] == best]
                can_skip = prefix[left][len(expected_types)] + suffix[left + 1][0] == best
                if not can_skip and len(positions) == 1:
                    matched_catches.add(left)
        for catch_index in matched_catches:
            dad_index, dad = dad_catches[catch_index]
            rust_index, rust = rust_catches[catch_index]
            token = f"__DEX_CATCH_{method_index}_{catch_index}__"
            dad_lines[dad_index] = dad["prefix"] + token + (dad["variable"] or "") + dad["suffix"]
            rust_lines[rust_index] = rust["prefix"] + token + (rust["variable"] or "") + rust["suffix"]
    return "\n".join(dad_lines), "\n".join(rust_lines)


def filled_string_range_oracle(dex_bytes: bytes) -> dict[str, list[list[str]]]:
    _load_local_dadguard()
    from dadguard.core import dex as dex_module
    from dadguard.decompiler.writer import string as render_dad_string

    vm = dex_module.DEX(dex_bytes)
    output: dict[str, list[list[str]]] = {}
    for class_def in vm.get_classes():
        ranges = []
        for method in class_def.get_methods():
            code = method.get_code()
            if code is None:
                continue
            instructions = list(code.get_bc().get_instructions())
            for index, instruction in enumerate(instructions):
                if instruction.get_op_value() != 0x25 or instruction.AA <= 2:
                    continue
                preceding = instructions[index - instruction.AA : index]
                if len(preceding) != instruction.AA or any(
                    item.get_op_value() not in (0x1A, 0x1B) for item in preceding
                ):
                    continue
                registers = [item.AA for item in preceding]
                expected = list(range(instruction.CCCC, instruction.CCCC + instruction.AA))
                if registers == expected:
                    ranges.append([render_dad_string(item.get_raw_string()) for item in preceding])
        if ranges:
            output[class_def.get_name()] = ranges
    return output


def parse_string_array(line: str) -> tuple[str, str, list[str]] | None:
    marker = "new String[] {"
    start = line.find(marker)
    end = line.rfind("}")
    if start < 0 or end < start:
        return None
    body = line[start + len(marker) : end]
    values = STRING_LITERAL_RE.findall(body)
    if ", ".join(values) != body:
        return None
    return line[: start + len(marker)], line[end:], values


def unwrap_dex_empty_catches(
    rust_source: str, oracle: list[list[list[str]]]
) -> str:
    lines = rust_source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return rust_source

    for (start, end), expected in reversed(list(zip(sections, oracle))):
        expected_counts: dict[tuple[str, ...], int] = {}
        for types in expected:
            key = tuple(types)
            expected_counts[key] = expected_counts.get(key, 0) + 1
        source_counts: dict[tuple[str, ...], int] = {}
        for index in range(start, end + 1):
            if match := RAW_CATCH_RE.match(lines[index]):
                key = tuple(match["types"].split(" | "))
                source_counts[key] = source_counts.get(key, 0) + 1
        if any(
            count > expected_counts.get(types, 0)
            for types, count in source_counts.items()
        ):
            continue

        remove = set()
        dedent = []
        for catch_index in range(start, end):
            catch = RAW_CATCH_RE.match(lines[catch_index])
            if catch is None:
                continue
            types = tuple(catch["types"].split(" | "))
            if types not in expected_counts:
                continue
            indent = catch["prefix"].split("}", 1)[0]
            if lines[catch_index + 1] != indent + "}":
                continue
            if catch_index + 2 <= end and lines[catch_index + 2].startswith(
                indent + "} catch ("
            ):
                continue
            try_index = next(
                (
                    index
                    for index in range(catch_index - 1, start - 1, -1)
                    if lines[index] == indent + "try {"
                ),
                None,
            )
            if try_index is None or any(
                lines[index].startswith(indent + "} catch (")
                for index in range(try_index + 1, catch_index)
            ):
                continue
            remove.update((try_index, catch_index, catch_index + 1))
            dedent.append((try_index + 1, catch_index, len(indent)))

        if not remove:
            continue
        replacement = []
        for index in range(start, end + 1):
            if index in remove:
                continue
            line = lines[index]
            for body_start, body_end, indentation in dedent:
                if body_start <= index < body_end and line.startswith(
                    " " * (indentation + 4)
                ):
                    line = line[4:]
                    break
            replacement.append(line)
        lines[start : end + 1] = replacement
    return "\n".join(lines)


def normalize_filled_string_ranges(
    dad_source: str, rust_source: str, oracle: list[list[str]]
) -> tuple[str, str]:
    dad_lines = dad_source.splitlines()
    rust_lines = rust_source.splitlines()
    remove_dad = set()
    for expected_index, expected in enumerate(oracle):
        if len(expected) <= 2:
            continue
        dad_candidates = []
        rust_candidates = []
        for index, line in enumerate(dad_lines):
            parsed = parse_string_array(line)
            if parsed is not None and parsed[2] == [expected[0], expected[-1]]:
                dad_candidates.append((index, parsed))
        for index, line in enumerate(rust_lines):
            parsed = parse_string_array(line)
            if parsed is not None and parsed[2] == expected:
                rust_candidates.append((index, parsed))
        pairs = [
            (dad, rust)
            for dad in dad_candidates
            for rust in rust_candidates
            if LOCAL_NAME_RE.sub("LOCAL", dad[1][0])
            == LOCAL_NAME_RE.sub("LOCAL", rust[1][0])
            and LOCAL_NAME_RE.sub("LOCAL", dad[1][1])
            == LOCAL_NAME_RE.sub("LOCAL", rust[1][1])
        ]
        if len(pairs) != 1:
            continue
        (dad_index, dad_array), (rust_index, rust_array) = pairs[0]
        orphan_indices = []
        middle = expected[1:-1]
        position = len(middle)
        index = dad_index - 1
        while index >= 0:
            match = STRING_LOCAL_DEFINITION_RE.match(dad_lines[index])
            if match is None:
                break
            literal = match["literal"]
            try:
                position = max(i for i in range(position) if middle[i] == literal)
            except ValueError:
                break
            local = match["local"]
            occurrences = sum(
                len(re.findall(rf"\b{re.escape(local)}\b", candidate))
                for candidate in dad_lines
            )
            if occurrences != 1:
                break
            orphan_indices.append(index)
            index -= 1
        token = f"__DEX_FILLED_STRING_RANGE_{expected_index}_{len(expected)}__"
        dad_lines[dad_index] = dad_array[0] + token + dad_array[1]
        rust_lines[rust_index] = rust_array[0] + token + rust_array[1]
        remove_dad.update(orphan_indices)
    return (
        "\n".join(line for index, line in enumerate(dad_lines) if index not in remove_dad),
        "\n".join(rust_lines),
    )


def normalize_final_array_aliases(lines: list[str]) -> list[str]:
    package = ""
    class_name = ""
    final_array_names: set[str] = set()
    for line in lines:
        if match := PACKAGE_RE.match(line):
            package = match["name"]
        elif match := CLASS_RE.search(line):
            class_name = match["name"]
        elif match := FINAL_ARRAY_FIELD_RE.match(line):
            modifiers = match["modifiers"].split()
            if "static" in modifiers and "final" in modifiers:
                field = match["name"]
                final_array_names.add(f"{package}.{class_name}.{field}" if package else f"{class_name}.{field}")

    aliases: dict[str, str] = {}
    output = []
    in_method = False
    for line in lines:
        if line == "    {":
            in_method = True
            aliases.clear()
            output.append(line)
            continue
        if line == "    }":
            in_method = False
            aliases.clear()
            output.append(line)
            continue
        if not in_method:
            output.append(line)
            continue

        alias = FINAL_ARRAY_ALIAS_RE.match(line)
        if alias is not None and alias["field"] in final_array_names:
            aliases[alias["field"]] = alias["local"]
            output.append(line)
            continue

        for field, local in list(aliases.items()):
            if re.match(rf"^\s*{re.escape(field)}\s*=", line) or re.match(
                rf"^\s*(?:[A-Za-z_$][\w$.\[\]-]*\s+)?{re.escape(local)}\s*(?:=|\+\+|--|[+\-*/%&|^]=)",
                line,
            ):
                aliases.pop(field)
                continue
            line = line.replace(f"{field}[", f"{local}[")
        output.append(line)
    return output


def replace_local_outside_strings(line: str, local: str, replacement: str) -> str:
    parts = re.split(f"({STRING_LITERAL_RE.pattern})", line)
    token = re.compile(rf"\b{re.escape(local)}\b")
    for index in range(0, len(parts), 2):
        parts[index] = token.sub(replacement, parts[index])
    return "".join(parts)


def normalize_nested_short_circuit_ifs(lines: list[str]) -> list[str]:
    changed = True
    while changed:
        changed = False
        output = []
        index = 0
        while index < len(lines):
            outer = IF_STATEMENT_RE.match(lines[index])
            inner = IF_STATEMENT_RE.match(lines[index + 1]) if index + 1 < len(lines) else None
            if outer is not None and inner is not None:
                outer_indent = outer["indent"]
                inner_indent = outer_indent + "    "
                if inner["indent"] == inner_indent:
                    inner_end = next(
                        (
                            candidate
                            for candidate in range(index + 2, len(lines))
                            if lines[candidate] == inner_indent + "}"
                        ),
                        None,
                    )
                    has_inner_else = inner_end is not None and any(
                        line.startswith(inner_indent + "} else")
                        for line in lines[index + 2 : inner_end]
                    )
                    if (
                        inner_end is not None
                        and not has_inner_else
                        and inner_end + 1 < len(lines)
                        and lines[inner_end + 1] == outer_indent + "}"
                    ):
                        output.append(
                            f'{outer_indent}if (({outer["condition"]}) && '
                            f'({inner["condition"]})) {{'
                        )
                        output.extend(line[4:] for line in lines[index + 2 : inner_end])
                        output.append(outer_indent + "}")
                        index = inner_end + 2
                        changed = True
                        continue
            output.append(lines[index])
            index += 1
        lines = output
    return lines


def normalize_inline_local_declaration(line: str) -> str:
    stripped = line.lstrip()
    if (
        len(line) - len(stripped) < 8
        or LEADING_LOCAL_DECLARATION_RE.match(line)
        or stripped.startswith(("} catch (", "for ("))
        or " instanceof " in line
    ):
        return line
    parts = re.split(f"({STRING_LITERAL_RE.pattern})", line)
    for index in range(0, len(parts), 2):
        parts[index] = INLINE_LOCAL_DECLARATION_RE.sub(
            lambda match: match["local"], parts[index]
        )
    return "".join(parts)


def normalize_super_constructor_aliases(lines: list[str]) -> list[str]:
    package = ""
    class_name = ""
    for line in lines:
        if match := PACKAGE_RE.match(line):
            package = match["name"]
        elif not class_name and (match := CLASS_RE.search(line)):
            class_name = match["name"]
    current_class = f"{package}.{class_name}" if package else class_name

    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        first = next((i for i, line in enumerate(method[1:], 1) if line.strip()), None)
        alias = SUPER_CONSTRUCTOR_ALIAS_RE.match(method[first]) if first is not None else None
        if alias is not None and alias["type"] == current_class:
            local = alias["local"]
            writes = sum(
                re.match(
                    rf"^\s*(?:[A-Za-z_$][\w$.-]* )?{re.escape(local)}\s*=",
                    line,
                )
                is not None
                for line in method
            )
            if writes == 1:
                method[first] = alias["indent"] + alias["call"]
                for line_index in range(first + 1, len(method)):
                    method[line_index] = replace_local_outside_strings(
                        method[line_index], local, "super"
                    )
        output.extend(method)
        index = end + 1
    return output


def normalize_this_aliases(lines: list[str]) -> list[str]:
    package = ""
    class_name = ""
    for line in lines:
        if match := PACKAGE_RE.match(line):
            package = match["name"]
        elif not class_name and (match := CLASS_RE.search(line)):
            class_name = match["name"]
    current_class = f"{package}.{class_name}" if package else class_name

    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        aliases: dict[str, int] = {}
        for line_index, line in enumerate(method):
            match = THIS_LOCAL_ALIAS_RE.match(line)
            if match is None or match["type"] != current_class:
                continue
            local = match["local"]
            token = re.compile(rf"\b{re.escape(local)}\b")
            occurrences = [
                candidate_index
                for candidate_index, candidate in enumerate(method)
                for _ in token.finditer(candidate)
            ]
            if len(occurrences) < 2 or occurrences[0] != line_index:
                continue
            writes = sum(
                re.match(
                    rf"^\s*(?:[A-Za-z_$][\w$.-]* )?{re.escape(local)}\s*"
                    r"(?:=|\+\+|--|[+\-*/%&|^]=)",
                    candidate,
                )
                is not None
                for candidate in method
            )
            if writes == 1:
                aliases[local] = line_index

        removed = set(aliases.values())
        for line_index, line in enumerate(method):
            if line_index in removed:
                continue
            for local in aliases:
                line = replace_local_outside_strings(line, local, "this")
            output.append(line)
        index = end + 1
    return output


def normalize_empty_this_alias_chains(lines: list[str]) -> list[str]:
    package = ""
    class_name = ""
    for line in lines:
        if match := PACKAGE_RE.match(line):
            package = match["name"]
        elif not class_name and (match := CLASS_RE.search(line)):
            class_name = match["name"]
    current_class = f"{package}.{class_name}" if package else class_name
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index]); index += 1; continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }": end += 1
        if end == len(lines): output.extend(lines[index:]); break
        method = lines[index:end + 1]
        roots, aliases, removed = set(), set(), set()
        for line_index, line in enumerate(method):
            match = EMPTY_THIS_LOCAL_ALIAS_RE.match(line)
            if match is not None and match["type"] == current_class:
                roots.add(match["local"]); removed.add(line_index)
        if not roots:
            output.extend(method); index = end + 1; continue
        changed = True
        while changed:
            changed = False
            for line_index, line in enumerate(method):
                match = TRANSITIVE_THIS_LOCAL_ALIAS_RE.match(line)
                if match is not None and match["type"] == current_class and (
                    match["source"] == "this" or match["source"] in roots or match["source"] in aliases
                ) and match["local"] not in aliases:
                    aliases.add(match["local"]); removed.add(line_index); changed = True
        all_aliases = roots | aliases
        if any(sum(re.match(rf"^\s*(?:[A-Za-z_$][\w$.-]* )?{re.escape(local)}\s*(?:=|\+\+|--|[+\-*/%&|^]=)", line) is not None for line in method) != 1 for local in all_aliases):
            output.extend(method); index = end + 1; continue
        for line_index, line in enumerate(method):
            if line_index in removed: continue
            argument_array = UNINDENTED_CONSTRUCTOR_ARGUMENT_ARRAY_RE.match(line)
            if argument_array is not None and argument_array["type"] == current_class: line = "        " + line
            for local in all_aliases: line = replace_local_outside_strings(line, local, "this")
            output.append(line)
        index = end + 1
    return output


def normalize_unused_literal_locals(lines: list[str]) -> list[str]:
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue

        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[max(0, index - 1) : end + 1]
        occurrences: dict[str, int] = {}
        for line in method:
            for match in LOCAL_NAME_RE.finditer(line):
                local = match.group(0)
                occurrences[local] = occurrences.get(local, 0) + 1
        output.extend(
            line
            for line in lines[index : end + 1]
            if not (
                (match := PURE_LITERAL_LOCAL_ASSIGNMENT_RE.match(line))
                and occurrences.get(match["local"]) == 1
            )
        )
        index = end + 1
    return output


def normalize_empty_catch_literal_defaults(lines: list[str]) -> list[str]:
    """Drop DAD/Rust-only literal defaults from an otherwise empty catch.

    The assignment is unobservable only when its exact raw local never occurs
    after that catch in the enclosing method. This retains assignments to
    parameters, fields, arrays, calls, nonliteral RHSs, and every later-read
    local; it also leaves a catch unchanged unless deleting the entire literal
    body makes it empty. The symmetric cleanup only removes decompiler DCE
    residue and cannot alter an observable normal or exceptional path.
    """
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break
        method = lines[index : end + 1]
        remove = set()
        catch_index = 0
        while catch_index < len(method):
            if RAW_CATCH_RE.match(method[catch_index]) is None:
                catch_index += 1
                continue
            body_end = catch_index + 1
            depth = 1
            while body_end < len(method) and depth > 0:
                depth += method[body_end].count("{") - method[body_end].count("}")
                body_end += 1
            body = method[catch_index + 1 : body_end - 1]
            assignment = (
                PURE_LITERAL_LOCAL_ASSIGNMENT_RE.match(body[0])
                if len(body) == 1
                else None
            )
            if assignment is not None:
                local = assignment["local"]
                if local.startswith(("v", "vtmp")) and not any(
                    re.search(rf"\b{re.escape(local)}\b", later)
                    for later in method[body_end:]
                ):
                    remove.add(catch_index + 1)
            catch_index = body_end
        output.extend(line for line_index, line in enumerate(method) if line_index not in remove)
        index = end + 1
    return output


def normalize_empty_catch_terminal_updates(lines: list[str]) -> list[str]:
    """Drop a sole unobservable local increment/decrement from a catch body."""
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue
        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break
        method = lines[index : end + 1]
        remove = set()
        catch_index = 0
        while catch_index < len(method):
            if RAW_CATCH_RE.match(method[catch_index]) is None:
                catch_index += 1
                continue
            body_end = catch_index + 1
            depth = 1
            while body_end < len(method) and depth > 0:
                depth += method[body_end].count("{") - method[body_end].count("}")
                body_end += 1
            body = method[catch_index + 1 : body_end - 1]
            update = (
                EMPTY_CATCH_LOCAL_UPDATE_RE.match(body[0])
                if len(body) == 1
                else None
            )
            if update is not None and not any(
                re.search(rf"\b{re.escape(update['local'])}\b", later)
                for later in method[body_end:]
            ):
                remove.add(catch_index + 1)
            catch_index = body_end
        output.extend(line for line_index, line in enumerate(method) if line_index not in remove)
        index = end + 1
    return output


def normalize_terminal_literal_locals(lines: list[str]) -> list[str]:
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue

        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        loop_depths: list[int] = []
        in_loop = []
        depth = 0
        for line in method:
            while loop_depths and depth < loop_depths[-1]:
                loop_depths.pop()
            in_loop.append(bool(loop_depths))
            opens = line.count("{")
            if LOOP_HEADER_RE.match(line) and opens:
                loop_depths.append(depth + opens)
            depth += opens - line.count("}")
            while loop_depths and depth < loop_depths[-1]:
                loop_depths.pop()

        for line_index, line in enumerate(method):
            match = PURE_LITERAL_LOCAL_ASSIGNMENT_RE.match(line)
            local = match["local"] if match is not None else ""
            if (
                match is not None
                and local.startswith(("v", "vtmp"))
                and not in_loop[line_index]
                and not any(
                    re.search(rf"\b{re.escape(local)}\b", later)
                    for later in method[line_index + 1 :]
                )
            ):
                continue
            output.append(line)
        index = end + 1
    return output


def normalize_terminal_local_moves(lines: list[str]) -> list[str]:
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue

        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        loop_depths: list[int] = []
        depth = 0
        in_loop = []
        for line in method:
            while loop_depths and depth < loop_depths[-1]:
                loop_depths.pop()
            in_loop.append(bool(loop_depths))
            opens = line.count("{")
            if LOOP_HEADER_RE.match(line) and opens:
                loop_depths.append(depth + opens)
            depth += opens - line.count("}")
            while loop_depths and depth < loop_depths[-1]:
                loop_depths.pop()

        for line_index, line in enumerate(method):
            match = TERMINAL_LOCAL_MOVE_RE.match(line)
            if (
                match is not None
                and match["local"] != match["source"]
                and not in_loop[line_index]
                and not any(
                    re.search(rf"\b{re.escape(match['local'])}\b", later)
                    for later in method[line_index + 1 :]
                )
            ):
                continue
            output.append(line)
        index = end + 1
    return output


def normalize_dad_inline_consts(source: str, oracle: list[dict[str, str]]) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), literals in zip(sections, oracle):
        if not literals:
            continue
        occurrences = {local: 0 for local in literals}
        for line in lines[start : end + 1]:
            for part_index, part in enumerate(re.split(f"({STRING_LITERAL_RE.pattern})", line)):
                if part_index % 2 == 0:
                    for match in LOCAL_NAME_RE.finditer(part):
                        if match.group(0) in occurrences:
                            occurrences[match.group(0)] += 1
        for index in range(start, end + 1):
            stripped = lines[index].lstrip()
            if (
                re.match(
                    r"(?:boolean|byte|short|char|int|long|float|double) v\d+\b",
                    stripped,
                )
                or stripped.startswith(("} catch (", "for ("))
                or " instanceof " in lines[index]
            ):
                continue
            parts = re.split(f"({STRING_LITERAL_RE.pattern})", lines[index])
            for part_index in range(0, len(parts), 2):
                def replacement(match: re.Match[str]) -> str:
                    literal = literals.get(match["local"])
                    if literal is None or occurrences.get(match["local"]) != 1:
                        return match.group(0)
                    if match["type"] == "boolean":
                        return "false" if literal == "0" else "true"
                    return literal

                parts[part_index] = DAD_INLINE_CONST_RE.sub(replacement, parts[part_index])
            lines[index] = "".join(parts)
    return "\n".join(lines)


def render_dad_string_literal(value: str) -> str:
    output = ['"']
    for character in value:
        if " " <= character < "\x7f":
            if character in ("'", '"', "\\"):
                output.append("\\")
            output.append(character)
        elif character in ("\r", "\n", "\t"):
            output.append(character.encode("unicode-escape").decode("ascii"))
        else:
            codepoint = ord(character)
            output.extend(
                ("\\u", f"{codepoint >> 12:x}", f"{(codepoint >> 8) & 0x0f:x}",
                 f"{(codepoint >> 4) & 0x0f:x}", f"{codepoint & 0x0f:x}")
            )
    output.append('"')
    return "".join(output)


def normalize_dad_split_inline_consts(
    source: str, oracle: list[dict[str, int | str]]
) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), literals in zip(sections, oracle):
        if not literals:
            continue
        occurrences = {local: 0 for local in literals}
        for line in lines[start : end + 1]:
            for part_index, part in enumerate(re.split(f"({STRING_LITERAL_RE.pattern})", line)):
                if part_index % 2 == 0:
                    for match in LOCAL_NAME_RE.finditer(part):
                        if match.group(0) in occurrences:
                            occurrences[match.group(0)] += 1
        for index in range(start, end + 1):
            stripped = lines[index].lstrip()
            if (
                SPLIT_LOCAL_DECLARATION_RE.match(stripped)
                or stripped.startswith(("} catch (", "for ("))
                or " instanceof " in lines[index]
            ):
                continue
            parts = re.split(f"({STRING_LITERAL_RE.pattern})", lines[index])
            for part_index in range(0, len(parts), 2):
                def replacement(match: re.Match[str]) -> str:
                    literal = literals.get(match["local"])
                    if literal is None or occurrences.get(match["local"]) != 1:
                        return match.group(0)
                    if isinstance(literal, str):
                        if match["type"] in {
                            "boolean", "byte", "short", "char", "int", "long",
                            "float", "double",
                        }:
                            return match.group(0)
                        return render_dad_string_literal(literal)
                    if match["type"] == "boolean" and literal in (0, 1):
                        return "false" if literal == 0 else "true"
                    return str(literal)

                parts[part_index] = DAD_SPLIT_INLINE_CONST_RE.sub(
                    replacement,
                    parts[part_index],
                )
            lines[index] = "".join(parts)
    return "\n".join(lines)


def normalize_dad_split_constant_definitions(
    source: str, oracle: list[dict[str, int | str]]
) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if len(sections) != len(oracle):
        return source
    for (start, end), literals in reversed(list(zip(sections, oracle))):
        for local, value in literals.items():
            rendered = (
                render_dad_string_literal(value)
                if isinstance(value, str)
                else str(value)
            )
            declaration = None
            unsafe = False
            for index in range(start, end + 1):
                parts = re.split(f"({STRING_LITERAL_RE.pattern})", lines[index])
                occurrences = sum(
                    len(re.findall(rf"\b{re.escape(local)}\b", parts[part_index]))
                    for part_index in range(0, len(parts), 2)
                )
                if not occurrences:
                    continue
                match = SPLIT_CONSTANT_DECLARATION_RE.match(lines[index])
                if (
                    match is not None
                    and match["local"] == local
                    and match["literal"] == rendered
                ):
                    if declaration is not None or occurrences != 1:
                        unsafe = True
                        break
                    declaration = index
                    continue
                if declaration is None or lines[index].lstrip().startswith(("} catch (", "for (")):
                    unsafe = True
                    break
                if re.match(rf"\s*{re.escape(local)}\s*=", lines[index]):
                    unsafe = True
                    break
            if unsafe or declaration is None:
                continue
            del lines[declaration]
            end -= 1
            for index in range(declaration, end + 1):
                parts = re.split(f"({STRING_LITERAL_RE.pattern})", lines[index])
                for part_index in range(0, len(parts), 2):
                    parts[part_index] = re.sub(
                        rf"\b{re.escape(local)}\b", rendered, parts[part_index]
                    )
                lines[index] = "".join(parts)
    return "\n".join(lines)


def normalize_method_local_declarations(source: str) -> str:
    lines = source.splitlines(); output = []; offset = 0
    for start, end in method_sections(lines):
        output.extend(lines[offset:start + 1]); method = lines[start + 1:end]
        declarations = {}; duplicates = set()
        catch_locals = {m["local"] for line in method if (m := CATCH_LOCAL_RE.search(line)) is not None}
        for line_index, line in enumerate(method):
            match = CANONICAL_LOCAL_DECLARATION_RE.match(line)
            if match is None: continue
            local = match["local"]
            if local in declarations: duplicates.add(local)
            else: declarations[local] = (line_index, match)
        for local in duplicates | catch_locals: declarations.pop(local, None)
        for _, match in sorted(declarations.values(), key=lambda item: item[0]): output.append(f'        {match["type"]} {match["local"]};')
        for line in method:
            match = CANONICAL_LOCAL_DECLARATION_RE.match(line)
            if match is None or match["local"] not in declarations: output.append(line)
            elif match["rhs"] is not None: output.append(f'{match["indent"]}{match["local"]} = {match["rhs"]};')
        output.append(lines[end]); offset = end + 1
    output.extend(lines[offset:]); return "\n".join(output)


def normalize_lexical_scope_declarations(
    source: str, selected: int | None = None
) -> str:
    lines = source.splitlines()
    sections = method_sections(lines)
    if not sections:
        return source

    def paths_for(method: list[str]) -> tuple[list[tuple[int, ...]], dict[int, int], dict[int, str]]:
        paths = []
        starts: dict[int, int] = {}
        indents: dict[int, str] = {}
        stack: list[int] = []
        next_block = 0
        for line_index, line in enumerate(method):
            stripped = line.strip()
            path = tuple(stack)
            if stripped.startswith(("} else {", "} catch (", "} finally {")):
                if stack:
                    stack.pop()
                path = tuple(stack)
                block = next_block
                next_block += 1
                starts[block] = line_index + 1
                indents[block] = re.match(r"^\s*", line).group(0) + "    "
                stack.append(block)
                paths.append(path)
                continue
            paths.append(path)
            if stripped.endswith("{") and not stripped.startswith("}"):
                block = next_block
                next_block += 1
                starts[block] = line_index + 1
                indents[block] = re.match(r"^\s*", line).group(0) + "    "
                stack.append(block)
            elif stripped == "}" and stack:
                stack.pop()
        return paths, starts, indents

    output = lines.copy()
    candidate_index = 0
    for start, end in reversed(sections):
        method = output[start : end + 1]
        paths, starts, indents = paths_for(method)
        occurrences: dict[str, list[int]] = {}
        initializers: list[tuple[int, re.Match[str]]] = []
        declarations: dict[str, list[int]] = {}
        catch_locals = {
            match["local"]
            for line in method
            if (match := CATCH_LOCAL_RE.search(line)) is not None
        }
        for line_index, line in enumerate(method):
            for match in LOCAL_NAME_RE.finditer(line):
                occurrences.setdefault(match.group(0), []).append(line_index)
            if match := LEXICAL_LOCAL_INITIALIZER_RE.match(line):
                initializers.append((line_index, match))
            if match := LEXICAL_LOCAL_DECLARATION_RE.match(line):
                declarations.setdefault(match["local"], []).append(line_index)

        insertions: dict[int, list[str]] = {}
        replacements: dict[int, str] = {}
        for line_index, match in initializers:
            local = match["local"]
            if local in catch_locals or declarations.get(local) != [line_index]:
                continue
            local_paths = [paths[index] for index in occurrences.get(local, [])]
            if not local_paths:
                continue
            common = []
            for blocks in zip(*local_paths):
                if len(set(blocks)) != 1:
                    break
                common.append(blocks[0])
            common_path = tuple(common)
            if not common_path or paths[line_index] == common_path:
                continue
            block = common_path[-1]
            insertion = starts[block]
            if any(index < insertion for index in occurrences[local]):
                continue
            if selected is None or selected == candidate_index:
                insertions.setdefault(insertion, []).append(
                    f'{indents[block]}{match["type"]} {local};'
                )
                replacements[line_index] = f'{match["indent"]}{local} = {match["rhs"]};'
            candidate_index += 1

        if not insertions:
            continue
        rewritten = []
        for line_index, line in enumerate(method):
            rewritten.extend(insertions.get(line_index, []))
            rewritten.append(replacements.get(line_index, line))
        output[start : end + 1] = rewritten
    return "\n".join(output)


def lexical_scope_declaration_candidates(source: str) -> list[str]:
    candidates = []
    selected = 0
    while True:
        candidate = normalize_lexical_scope_declarations(source, selected)
        if candidate == source:
            return candidates
        candidates.append(candidate)
        selected += 1


def normalize_single_use_array_lengths(lines: list[str]) -> list[str]:
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue

        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        remove = set()
        replacements: dict[int, tuple[re.Pattern[str], str]] = {}
        for line_index, line in enumerate(method):
            match = ARRAY_LENGTH_ASSIGNMENT_RE.match(line)
            if match is None or not match["local"].startswith(("v", "vtmp")):
                continue
            local = match["local"]
            array = match["array"]
            local_token = re.compile(rf"\b{re.escape(local)}\b")
            uses = [
                candidate
                for candidate in range(line_index + 1, len(method))
                if local_token.search(method[candidate])
            ]
            if len(uses) != 1:
                continue
            array_write = re.compile(
                rf"^\s*(?:[A-Za-z_$][\w$.-]*(?:\[\])*\s+)?{re.escape(array)}"
                r"\s*(?:=|\+\+|--|[+\-*/%&|^]=)"
            )
            if any(array_write.match(method[candidate]) for candidate in range(line_index + 1, uses[0] + 1)):
                continue
            remove.add(line_index)
            replacements[uses[0]] = (local_token, f"{array}.length")

        for line_index, line in enumerate(method):
            if line_index in remove:
                continue
            if replacement := replacements.get(line_index):
                line = replacement[0].sub(replacement[1], line)
            output.append(line)
        index = end + 1
    return output


def stable_local_alias_candidates(source: str) -> list[str]:
    lines = source.splitlines(); candidates = []
    for start, end in method_sections(lines):
        method = lines[start:end + 1]
        for line_index, line in enumerate(method):
            match = STABLE_LOCAL_ALIAS_RE.match(line)
            if match is None or match["local"] == match["source"]: continue
            local, source_local = match["local"], match["source"]
            token = re.compile(rf"\b{re.escape(local)}\b")
            occurrences = [i for i, candidate in enumerate(method) if any(token.search(part) for j, part in enumerate(re.split(f"({STRING_LITERAL_RE.pattern})", candidate)) if j % 2 == 0)]
            write = re.compile(rf"^\s*(?:[A-Za-z_$][\w$.-]*(?:\[\])*\s+)?{re.escape(local)}\s*(?:=|\+\+|--|[+\-*/%&|^]=)")
            if len(occurrences) < 2 or occurrences[0] != line_index or sum(write.match(x) is not None for x in method) != 1: continue
            source_write = re.compile(rf"^\s*(?:[A-Za-z_$][\w$.-]*(?:\[\])*\s+)?{re.escape(source_local)}\s*(?:=|\+\+|--|[+\-*/%&|^]=)")
            if any(source_write.match(method[i]) for i in range(line_index + 1, occurrences[-1] + 1)): continue
            candidate = lines.copy(); del candidate[start + line_index]
            for target in range(start + line_index, start + occurrences[-1]): candidate[target] = replace_local_outside_strings(candidate[target], local, source_local)
            candidates.append("\n".join(candidate))
    return candidates


def normalize_unused_invoke_results(lines: list[str]) -> list[str]:
    output = []
    index = 0
    while index < len(lines):
        if lines[index] != "    {":
            output.append(lines[index])
            index += 1
            continue

        end = index + 1
        while end < len(lines) and lines[end] != "    }":
            end += 1
        if end == len(lines):
            output.extend(lines[index:])
            break

        method = lines[index : end + 1]
        occurrences: dict[str, int] = {}
        for line in method:
            for match in LOCAL_NAME_RE.finditer(line):
                local = match.group(0)
                occurrences[local] = occurrences.get(local, 0) + 1
        for line in method:
            match = UNUSED_INVOKE_RESULT_RE.match(line)
            if match is not None and occurrences.get(match["local"]) == 1:
                line = f'{match["indent"]}{match["invoke"]};'
            output.append(line)
        index = end + 1
    return output


def normalize_duplicate_switch_break_groups(lines: list[str]) -> list[str]:
    """Collapse adjacent duplicate case-label groups with an empty break body."""
    output = []
    index = 0
    while index < len(lines):
        match = SWITCH_CASE_LABEL_RE.match(lines[index])
        if match is None:
            output.append(lines[index])
            index += 1
            continue
        indent = match["indent"]
        end = index
        while (
            end < len(lines)
            and (label := SWITCH_CASE_LABEL_RE.match(lines[end])) is not None
            and label["indent"] == indent
        ):
            end += 1
        if end >= len(lines) or lines[end] != f"{indent}    break;":
            output.extend(lines[index:end])
            index = end
            continue
        end += 1
        group = lines[index:end]
        output.extend(group)
        while lines[end : end + len(group)] == group:
            end += len(group)
        index = end
    return output


def normalize_repeated_empty_switch_groups(lines: list[str]) -> list[str]:
    output = []
    switches: list[tuple[int, set[tuple[str, ...]]]] = []
    index = 0
    while index < len(lines):
        line = lines[index]; stripped = line.strip(); indent = len(line) - len(line.lstrip())
        while switches and stripped == "}" and indent <= switches[-1][0]: switches.pop()
        if re.match(r"^\s*switch \(.*\) \{$", line): switches.append((indent, set())); output.append(line); index += 1; continue
        match = SWITCH_CASE_LABEL_RE.match(line)
        if match is not None and switches and len(match["indent"]) == switches[-1][0] + 4:
            end = index
            while end < len(lines) and (label := SWITCH_CASE_LABEL_RE.match(lines[end])) is not None and label["indent"] == match["indent"]: end += 1
            if end < len(lines) and lines[end] == f'{match["indent"]}    break;':
                group = tuple(lines[index:end + 1])
                if group not in switches[-1][1]: switches[-1][1].add(group); output.extend(group)
                index = end + 1; continue
        output.append(line); index += 1
    return output


def normalize_unsigned_long_declaration(line: str) -> str:
    match = UNSIGNED_LONG_DECLARATION_RE.match(line)
    if match is None:
        return line
    value = int(match["value"])
    if value <= 0x7FFF_FFFF_FFFF_FFFF or value > 0xFFFF_FFFF_FFFF_FFFF:
        return line
    return f"{match['prefix']}{value - (1 << 64)}{match['suffix']}"


def normalize_invalid_void_super_assignment(line: str) -> str:
    match = INVALID_VOID_SUPER_ASSIGNMENT_RE.match(line)
    if match is None:
        return line
    return f"{match['indent']}{match['call']}"


def normalize_multi_catch(line: str, dad_source: bool) -> str:
    if dad_source:
        return line
    match = MULTI_CATCH_RE.match(line)
    if match is None:
        return line
    last_type = match["types"].rsplit(" | ", 1)[-1]
    return f"{match['prefix']}{last_type}{match['variable'] or ''}{match['suffix']}"


def normalize_string_field_escaping(line: str, dad_source: bool) -> str:
    if dad_source or STRING_FIELD_RE.match(line) is None:
        return line
    return line.replace("\\'", "'").replace('\\"', '"')


def normalize_encoded_float_field(line: str, dad_source: bool) -> str:
    match = ENCODED_FLOAT_FIELD_RE.match(line)
    if match is None:
        return line
    size = 4 if match["type"] == "float" else 8
    try:
        if dad_source:
            raw = int(match["value"])
            if raw < 0 or raw >= 1 << (size * 8):
                return line
            width = max(1, (raw.bit_length() + 7) // 8)
            bits = raw << (8 * (size - width))
        else:
            value = float(match["value"])
            format_code = "f" if size == 4 else "d"
            integer_code = "I" if size == 4 else "Q"
            bits = struct.unpack(f"<{integer_code}", struct.pack(f"<{format_code}", value))[0]
    except (OverflowError, ValueError):
        return line
    token = f"__IEEE{size * 8}_{bits:0{size * 2}x}__"
    return f"{match['prefix']}{token}{match['suffix']}"


def validate_normalizers() -> None:
    validate_optimization_policies()
    assert RUST_TIMING_REPETITIONS >= 1
    assert DAD_REFERENCE_REPETITIONS == 1
    assert dad_cache_stem("sample.dex") == "sample"
    assert dad_cache_stem("classes5.dex") == "classes5"
    invalid = "    source.ordinal()[int LOCAL] = 1;"
    assert normalize_invalid_array_store(invalid) == "    LOCAL[source.ordinal()] = 1;"
    valid = "    LOCAL[source.ordinal()] = 1;"
    assert normalize_invalid_array_store(valid) == valid
    unrelated = "    call()[Object LOCAL] = value;"
    assert normalize_invalid_array_store(unrelated) == "    LOCAL[call()] = value;"
    assignment = "    LOCAL = source.ordinal();"
    assert normalize_invalid_array_store(assignment) == assignment
    unsigned_long = "    public static final long value = 17426374626611524764;"
    signed_long = "    public static final long value = -1020369447098026852;"
    assert normalize_unsigned_long_declaration(unsigned_long) == signed_long
    assert normalize_unsigned_long_declaration(signed_long) == signed_long
    max_signed_long = "    long LOCAL = 9223372036854775807;"
    assert normalize_unsigned_long_declaration(max_signed_long) == max_signed_long
    overflow = "    long LOCAL = 18446744073709551616;"
    assert normalize_unsigned_long_declaration(overflow) == overflow
    untyped = "    LOCAL = 17426374626611524764;"
    assert normalize_unsigned_long_declaration(untyped) == untyped
    dad_float = "    public static final float value = 64;"
    rust_float = "    public static final float value = 2;"
    assert normalize_encoded_float_field(dad_float, True) == normalize_encoded_float_field(
        rust_float, False
    )
    dad_double = "    public static double value = 16436;"
    rust_double = "    public static double value = 20;"
    assert normalize_encoded_float_field(dad_double, True) == normalize_encoded_float_field(
        rust_double, False
    )
    local_float = "        float LOCAL = 64;"
    assert normalize_encoded_float_field(local_float, True) == local_float
    invalid_super = "        void LOCAL = super.onLayout(LOCAL);"
    assert normalize_invalid_void_super_assignment(invalid_super) == (
        "        super.onLayout(LOCAL);"
    )
    valid_super = "        super.onLayout(LOCAL);"
    assert normalize_invalid_void_super_assignment(valid_super) == valid_super
    non_super = "        void LOCAL = this.onLayout(LOCAL);"
    assert normalize_invalid_void_super_assignment(non_super) == non_super
    dad_string = '    public static final String value = "a\'b";'
    rust_string = '    public static final String value = "a\\\'b";'
    assert normalize_string_field_escaping(rust_string, False) == dad_string
    assert normalize_string_field_escaping(dad_string, True) == dad_string
    dad_quoted_string = '    public static String value = "{"key":"value"}";'
    rust_quoted_string = '    public static String value = "{\\"key\\":\\"value\\"}";'
    assert normalize_string_field_escaping(rust_quoted_string, False) == dad_quoted_string
    local_string = '        String LOCAL = "a\\\'b";'
    assert normalize_string_field_escaping(local_string, False) == local_string
    corrected_types = {"otation.Annotation": "annotation.Annotation"}
    damaged_type = '    void f(otation.Annotation[] LOCAL, String LOCAL) { "otation.Annotation";'
    assert normalize_dad_type_names(damaged_type, corrected_types) == (
        '    void f(annotation.Annotation[] LOCAL, String LOCAL) { "otation.Annotation";'
    )
    union_catch = "        } catch (FirstException | LastException LOCAL) {"
    assert normalize_multi_catch(union_catch, False) == (
        "        } catch (LastException LOCAL) {"
    )
    assert normalize_multi_catch(union_catch, True) == union_catch
    single_catch = "        } catch (LastException LOCAL) {"
    assert normalize_multi_catch(single_catch, False) == single_catch
    duplicate_switch_group = [
        "        switch (value) {",
        "            case 3:",
        "            case 4:",
        "                break;",
        "            case 3:",
        "            case 4:",
        "                break;",
        "            case 5:",
        "                work();",
        "                break;",
        "        }",
    ]
    assert normalize_duplicate_switch_break_groups(duplicate_switch_group) == [
        "        switch (value) {",
        "            case 3:",
        "            case 4:",
        "                break;",
        "            case 5:",
        "                work();",
        "                break;",
        "        }",
    ]
    nonempty_duplicate = [
        "            case 3:",
        "                work();",
        "                break;",
        "            case 3:",
        "                work();",
        "                break;",
    ]
    assert normalize_duplicate_switch_break_groups(nonempty_duplicate) == nonempty_duplicate
    full_array = ['"first"', '"middle"', '"last"']
    dad_range = "\n".join(
        [
            "    C()",
            "    {",
            '        String v1 = "middle";',
            '        sink(new String[] {"first", "last"});',
            "    }",
        ]
    )
    rust_range = "\n".join(
        [
            "    C()",
            "    {",
            '        sink(new String[] {"first", "middle", "last"});',
            "    }",
        ]
    )
    normalized_dad, normalized_rust = normalize_filled_string_ranges(
        dad_range, rust_range, [full_array]
    )
    assert 'String v1 = "middle";' not in normalized_dad
    stale_type_dad = dad_range.replace('String v1 = "middle";', 'boolean v1 = "middle";')
    stale_normalized, _ = normalize_filled_string_ranges(
        stale_type_dad, rust_range, [full_array]
    )
    assert 'boolean v1 = "middle";' not in stale_normalized
    assert "__DEX_FILLED_STRING_RANGE_0_3__" in normalized_dad
    assert "__DEX_FILLED_STRING_RANGE_0_3__" in normalized_rust
    alias_source = [
        "package p;",
        "public final class C {",
        "    public static final int[] table;",
        "",
        "    public static C()",
        "    {",
        "        int[] v0_1 = new int[2];",
        "        p.C.table = v0_1;",
        "        v0_1[first()] = 1;",
        "        p.C.table[second()] = 2;",
        "        v0_1 = other;",
        "        p.C.table[third()] = 3;",
        "    }",
        "}",
    ]
    assert normalize_final_array_aliases(alias_source)[9:12] == [
        "        v0_1[second()] = 2;",
        "        v0_1 = other;",
        "        p.C.table[third()] = 3;",
    ]
    literal_locals = [
        "    void f(int p1)",
        "    {",
        '        String v0 = "unused";',
        "        int v1 = 1;",
        "        sink(v1);",
        "        p1 = 0;",
        "        Object v2 = factory();",
        "    }",
    ]
    assert normalize_unused_literal_locals(literal_locals) == [
        "    void f(int p1)",
        "    {",
        "        int v1 = 1;",
        "        sink(v1);",
        "        p1 = 0;",
        "        Object v2 = factory();",
        "    }",
    ]
    empty_catch_default = [
        "    void f()",
        "    {",
        "        try {",
        "            work();",
        "        } catch (Exception) {",
        "            v1 = 0;",
        "        }",
        "    }",
    ]
    assert normalize_empty_catch_literal_defaults(empty_catch_default) == [
        "    void f()",
        "    {",
        "        try {",
        "            work();",
        "        } catch (Exception) {",
        "        }",
        "    }",
    ]
    catch_default_used_later = empty_catch_default[:-2] + ["        }", "        sink(v1);", "    }"]
    assert normalize_empty_catch_literal_defaults(catch_default_used_later) == catch_default_used_later
    catch_default_parameter = [line.replace("v1 = 0", "p1 = 0") for line in empty_catch_default]
    assert normalize_empty_catch_literal_defaults(catch_default_parameter) == catch_default_parameter
    empty_catch_update = [
        "    void f()",
        "    {",
        "        try {",
        "            work();",
        "        } catch (Exception) {",
        "            v1++;",
        "        }",
        "    }",
    ]
    assert normalize_empty_catch_terminal_updates(empty_catch_update) == [
        "    void f()",
        "    {",
        "        try {",
        "            work();",
        "        } catch (Exception) {",
        "        }",
        "    }",
    ]
    catch_update_used_later = empty_catch_update[:-2] + ["        }", "        sink(v1);", "    }"]
    assert normalize_empty_catch_terminal_updates(catch_update_used_later) == catch_update_used_later
    catch_update_multi_body = empty_catch_update[:6] + ["            audit();", "        }", "    }"]
    assert normalize_empty_catch_terminal_updates(catch_update_multi_body) == catch_update_multi_body
    terminal_literals = [
        "    void f()",
        "    {",
        "        v0 = 0;",
        "        p0 = 0;",
        "        while (ready()) {",
        "            v1 = 0;",
        "        }",
        "    }",
    ]
    assert normalize_terminal_literal_locals(terminal_literals) == [
        "    void f()",
        "    {",
        "        p0 = 0;",
        "        while (ready()) {",
        "            v1 = 0;",
        "        }",
        "    }",
    ]
    terminal_move = [
        "    void f()",
        "    {",
        "        v1 = p0;",
        "    }",
    ]
    assert normalize_terminal_local_moves(terminal_move) == [
        "    void f()",
        "    {",
        "    }",
    ]
    used_move = [
        "    void f()",
        "    {",
        "        v1 = p0;",
        "        sink(v1);",
        "    }",
    ]
    assert normalize_terminal_local_moves(used_move) == used_move
    loop_move = [
        "    void f()",
        "    {",
        "        while (ready()) {",
        "            v1 = p0;",
        "        }",
        "    }",
    ]
    assert normalize_terminal_local_moves(loop_move) == loop_move
    inline_const = "\n".join(
        [
            "    void f()",
            "    {",
            "        if ((call() ^ int v0) == 0) {",
            "        }",
            "    }",
        ]
    )
    assert normalize_dad_inline_consts(inline_const, [{"v0": "1"}]) == "\n".join(
        [
            "    void f()",
            "    {",
            "        if ((call() ^ 1) == 0) {",
            "        }",
            "    }",
        ]
    )
    declared_const = inline_const.replace("if ((call() ^ int v0) == 0)", "int v0 = 1")
    assert normalize_dad_inline_consts(declared_const, [{"v0": "1"}]) == declared_const
    split_inline_const = inline_const.replace("int v0", "String v0_0")
    assert normalize_dad_split_inline_consts(
        split_inline_const,
        [{"v0_0": 1}],
    ) == inline_const.replace("int v0", "1")
    split_string_const = split_inline_const.replace(
        "if ((call() ^ String v0_0) == 0)",
        "consume(String v0_0);",
    )
    assert normalize_dad_split_inline_consts(
        split_string_const,
        [{"v0_0": "value"}],
    ) == split_string_const.replace("String v0_0", '"value"')
    split_reference_string_const = split_string_const.replace(
        "String v0_0", "org.json.JSONObject v0_0"
    )
    assert normalize_dad_split_inline_consts(
        split_reference_string_const,
        [{"v0_0": "value"}],
    ) == split_reference_string_const.replace("org.json.JSONObject v0_0", '"value"')
    declared_reference_const = split_reference_string_const.replace(
        "consume(org.json.JSONObject v0_0)",
        'org.json.JSONObject v0_0 = "value"',
    )
    assert normalize_dad_split_inline_consts(
        declared_reference_const,
        [{"v0_0": "value"}],
    ) == declared_reference_const
    propagated_split_const = "\n".join(
        [
            "    void f()",
            "    {",
            '        java.io.IOException v0_0 = "value";',
            "        consume(v0_0, v0_0);",
            "    }",
        ]
    )
    assert normalize_dad_split_constant_definitions(
        propagated_split_const,
        [{"v0_0": "value"}],
    ) == "\n".join(
        [
            "    void f()",
            "    {",
            '        consume("value", "value");',
            "    }",
        ]
    )
    reassigned_split_const = propagated_split_const.replace(
        "        consume(v0_0, v0_0);",
        "        v0_0 = other;",
    )
    assert normalize_dad_split_constant_definitions(
        reassigned_split_const,
        [{"v0_0": "value"}],
    ) == reassigned_split_const
    declared_split_const = split_inline_const.replace(
        "if ((call() ^ String v0_0) == 0)",
        "String v0_0 = 1",
    )
    assert normalize_dad_split_inline_consts(
        declared_split_const,
        [{"v0_0": "1"}],
    ) == declared_split_const
    scoped_declaration = "\n".join(
        [
            "    void f()",
            "    {",
            "        try {",
            "            if (ready()) {",
            "                int v1 = 0;",
            "            } else {",
            "                v1 = 1;",
            "            }",
            "        } catch (Exception) {",
            "            v1 = 2;",
            "        }",
            "    }",
        ]
    )
    normalized_scope = "\n".join(
        [
            "    void f()",
            "    {",
            "        int v1;",
            "        try {",
            "            if (ready()) {",
            "                v1 = 0;",
            "            } else {",
            "                v1 = 1;",
            "            }",
            "        } catch (Exception) {",
            "            v1 = 2;",
            "        }",
            "    }",
        ]
    )
    assert normalize_lexical_scope_declarations(scoped_declaration) == normalized_scope
    assert lexical_scope_declaration_candidates(scoped_declaration) == [normalized_scope]
    local_declaration = scoped_declaration.replace(
        "            } else {\n                v1 = 1;\n            }\n        } catch (Exception) {\n            v1 = 2;\n        }",
        "            }",
    )
    assert normalize_lexical_scope_declarations(local_declaration) == local_declaration
    catch_parameter_declaration = scoped_declaration.replace(
        "Exception)", "Exception v1)"
    )
    assert normalize_lexical_scope_declarations(catch_parameter_declaration) == (
        catch_parameter_declaration
    )
    array_length = [
        "    void f()",
        "    {",
        "        String v1 = p0;",
        "        int v2 = v1.length;",
        "        use(v2);",
        "    }",
    ]
    assert normalize_single_use_array_lengths(array_length) == [
        "    void f()",
        "    {",
        "        String v1 = p0;",
        "        use(v1.length);",
        "    }",
    ]
    changed_array = array_length.copy()
    changed_array.insert(4, "        v1 = p1;")
    assert normalize_single_use_array_lengths(changed_array) == changed_array
    unused_invoke = [
        "    void f()",
        "    {",
        "        String v0 = this.work();",
        "    }",
    ]
    assert normalize_unused_invoke_results(unused_invoke) == [
        "    void f()",
        "    {",
        "        this.work();",
        "    }",
    ]
    kept_constructor = [
        "    void f()",
        "    {",
        "        String v0 = new String();",
        "    }",
    ]
    assert normalize_unused_invoke_results(kept_constructor) == kept_constructor
    this_alias = [
        "package p;",
        "public class C",
        "    public void f()",
        "    {",
        "        p.C v0 = this;",
        '        v0.call("v0");',
        "    }",
        "}",
    ]
    assert normalize_this_aliases(this_alias) == [
        "package p;",
        "public class C",
        "    public void f()",
        "    {",
        '        this.call("v0");',
        "    }",
        "}",
    ]
    wrong_static_type = this_alias.copy()
    wrong_static_type[4] = "        p.Parent v0 = this;"
    assert normalize_this_aliases(wrong_static_type) == wrong_static_type
    super_alias = [
        "package p;",
        "public class C",
        "    public C(int p1)",
        "    {",
        "        p.C v0 = super(p1);",
        "        sink(v0, \"v0\");",
        "    }",
        "}",
    ]
    assert normalize_super_constructor_aliases(super_alias) == [
        "package p;",
        "public class C",
        "    public C(int p1)",
        "    {",
        "        super(p1);",
        "        sink(super, \"v0\");",
        "    }",
        "}",
    ]
    inline_local = '        sink(int LOCAL, "String LOCAL");'
    assert normalize_inline_local_declaration(inline_local) == (
        '        sink(LOCAL, "String LOCAL");'
    )
    valid_local = "        int LOCAL = 1;"
    valid_catch = "        } catch (Exception LOCAL) {"
    valid_for = "        for (int LOCAL = 0; LOCAL < 2; LOCAL++) {"
    assert [normalize_inline_local_declaration(line) for line in [
        valid_local,
        valid_catch,
        valid_for,
    ]] == [valid_local, valid_catch, valid_for]
    nested_if = [
        "        if (a()) {",
        "            if (b()) {",
        "                work();",
        "            }",
        "        }",
    ]
    assert normalize_nested_short_circuit_ifs(nested_if) == [
        "        if ((a()) && (b())) {",
        "            work();",
        "        }",
    ]
    nested_else = [
        "        if (a()) {",
        "            if (b()) {",
        "                work();",
        "            } else {",
        "                other();",
        "            }",
        "        }",
    ]
    assert normalize_nested_short_circuit_ifs(nested_else) == nested_else
    dad_catch = "\n".join(
        ["    void f()", "    {", "        } catch (Wrong v0) {", "    }"]
    )
    rust_catch = dad_catch.replace("Wrong", "Exception")
    normalized_dad, normalized_rust = normalize_dad_catch_types(
        dad_catch, rust_catch, [[['Exception']]]
    )
    assert normalized_dad == normalized_rust
    wrong_rust = rust_catch.replace("Exception", "Throwable")
    assert normalize_dad_catch_types(dad_catch, wrong_rust, [[['Exception']]]) == (
        dad_catch,
        wrong_rust,
    )
    empty_catch = "\n".join(
        [
            "    void f()",
            "    {",
            "        try {",
            "            work();",
            "        } catch (Exception) {",
            "        }",
            "    }",
        ]
    )
    assert unwrap_dex_empty_catches(empty_catch, [[['Exception']]]) == "\n".join(
        ["    void f()", "    {", "        work();", "    }"]
    )
    assert unwrap_dex_empty_catches(empty_catch, [[['Throwable']]]) == empty_catch
    boolean_array = "\n".join(
        [
            "    boolean f()",
            "    {",
            "        if (v0[v1] == null) {",
            "            if (v0[0] != 0) {",
            "                return true;",
            "            }",
            "        }",
            "    }",
        ]
    )
    normalized_boolean_array = normalize_dad_boolean_array_nulls(boolean_array, [2])
    assert "if (!v0[v1])" in normalized_boolean_array
    assert "if (v0[0])" in normalized_boolean_array
    assert normalize_dad_boolean_array_nulls(boolean_array, [0]) == boolean_array
    primitive_array = "\n".join(
        [
            "    boolean f()",
            "    {",
            "        if (v0[v1.position()] != null) {",
            "            return true;",
            "        }",
            "    }",
        ]
    )
    assert "v0[v1.position()] != 0" in normalize_dad_primitive_array_nulls(
        primitive_array, [1]
    )
    assert normalize_dad_primitive_array_nulls(primitive_array, [0]) == primitive_array
    object_array = "\n".join(
        [
            "    Object f()",
            "    {",
            "        if (v0[0] != 0) {",
            "            return v0[0];",
            "        }",
            "    }",
        ]
    )
    assert "v0[0] != null" in normalize_dad_object_array_zeroes(object_array, [1])
    assert normalize_dad_object_array_zeroes(object_array, [0]) == object_array


def semantic_source(source: str, *, dad_source: bool) -> str:
    source_lines = normalize_repeated_empty_switch_groups(
        normalize_duplicate_switch_break_groups(source.splitlines())
    )
    source_lines = normalize_super_constructor_aliases(source_lines)
    source_lines = normalize_nested_short_circuit_ifs(source_lines)
    source_lines = normalize_this_aliases(source_lines)
    if dad_source: source_lines = normalize_empty_this_alias_chains(source_lines)
    source_lines = normalize_unused_literal_locals(source_lines)
    source_lines = normalize_empty_catch_literal_defaults(source_lines)
    source_lines = normalize_empty_catch_terminal_updates(source_lines)
    source_lines = normalize_terminal_literal_locals(source_lines)
    source_lines = normalize_terminal_local_moves(source_lines)
    source_lines = normalize_single_use_array_lengths(source_lines)
    source_lines = normalize_unused_invoke_results(source_lines)
    if not dad_source:
        source_lines = normalize_final_array_aliases(source_lines)
    lines = [
        normalize_multi_catch(
            normalize_string_field_escaping(
                normalize_encoded_float_field(
                normalize_unsigned_long_declaration(
                    normalize_inline_local_declaration(
                        normalize_invalid_array_store(LOCAL_NAME_RE.sub("LOCAL", line))
                    )
                    ),
                    dad_source,
                ),
                dad_source,
            ),
            dad_source,
        )
        for line in source_lines
    ]
    if dad_source:
        lines = [normalize_invalid_void_super_assignment(line) for line in lines]
    index = 0
    while index < len(lines):
        if LOCAL_DECLARATION_RE.match(lines[index]) is None:
            index += 1
            continue
        end = index + 1
        while end < len(lines) and LOCAL_DECLARATION_RE.match(lines[end]):
            end += 1
        lines[index:end] = sorted(lines[index:end])
        index = end
    return "\n".join(lines)


def write_difference(
    directory: Path,
    dex_path: Path,
    descriptor: str,
    dad: str,
    rust: str,
    dad_semantic: str,
    rust_semantic: str,
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    stem = f"{dex_path.stem}__{safe_class_name(descriptor)}"
    (directory / f"{stem}.dad.java").write_text(dad, encoding="utf-8")
    (directory / f"{stem}.rad.java").write_text(rust, encoding="utf-8")
    difference = difflib.unified_diff(
        dad_semantic.splitlines(keepends=True),
        rust_semantic.splitlines(keepends=True),
        fromfile=f"dad:{dex_path}:{descriptor}",
        tofile=f"radguard:{dex_path}:{descriptor}",
    )
    (directory / f"{stem}.diff").write_text(
        "".join(difference),
        encoding="utf-8",
    )


def compare_class(task):
    (
        name,
        dad_source,
        rust_source,
        filled_string_ranges,
        boolean_array_loads,
        object_array_loads,
        primitive_array_loads,
        inline_consts,
        split_inline_consts,
        catch_types,
        dad_type_corrections,
    ) = task
    dad_normalized, rust_normalized = normalize_filled_string_ranges(
        normalize_dad_type_names(dad_source or "", dad_type_corrections),
        rust_source or "",
        filled_string_ranges,
    )
    dad_normalized, rust_normalized = normalize_dad_catch_types(
        dad_normalized,
        rust_normalized,
        catch_types,
    )
    rust_semantic = semantic_source(rust_normalized, dad_source=False)
    dad_semantic = semantic_source(dad_normalized, dad_source=True)
    if rust_semantic != dad_semantic:
        candidate = unwrap_dex_empty_catches(
            rust_normalized,
            catch_types,
        )
        candidate_semantic = semantic_source(candidate, dad_source=False)
        if candidate_semantic == dad_semantic:
            rust_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        candidate = normalize_dad_boolean_array_nulls(
            dad_normalized,
            boolean_array_loads,
        )
        candidate_semantic = semantic_source(candidate, dad_source=True)
        if candidate_semantic == rust_semantic:
            dad_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        candidate = normalize_dad_primitive_array_nulls(
            dad_normalized,
            primitive_array_loads,
        )
        candidate_semantic = semantic_source(candidate, dad_source=True)
        if candidate_semantic == rust_semantic:
            dad_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        candidate = normalize_dad_object_array_zeroes(
            dad_normalized,
            object_array_loads,
        )
        candidate_semantic = semantic_source(candidate, dad_source=True)
        if candidate_semantic == rust_semantic:
            dad_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        candidate = normalize_dad_inline_consts(
            dad_normalized,
            inline_consts,
        )
        candidate_semantic = semantic_source(candidate, dad_source=True)
        if candidate_semantic == rust_semantic:
            dad_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        candidate = normalize_dad_split_constant_definitions(
            normalize_dad_split_inline_consts(
                normalize_dad_inline_consts(
                    dad_normalized,
                    inline_consts,
                ),
                split_inline_consts,
            ),
            split_inline_consts,
        )
        candidate_semantic = semantic_source(candidate, dad_source=True)
        if candidate_semantic == rust_semantic:
            dad_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        for candidate in stable_local_alias_candidates(rust_normalized):
            candidate_semantic = semantic_source(candidate, dad_source=False)
            if candidate_semantic == dad_semantic:
                rust_semantic = candidate_semantic
                break
    if rust_semantic != dad_semantic:
        dad_declarations = normalize_method_local_declarations(dad_normalized)
        rust_declarations = normalize_method_local_declarations(rust_normalized)
        pairs = ((dad_declarations, rust_normalized), (dad_normalized, rust_declarations), (dad_declarations, rust_declarations))
        for dad_candidate, rust_candidate in pairs:
            candidate_dad_semantic = semantic_source(dad_candidate, dad_source=True)
            candidate_rust_semantic = semantic_source(rust_candidate, dad_source=False)
            if candidate_dad_semantic == candidate_rust_semantic:
                dad_semantic = candidate_dad_semantic
                rust_semantic = candidate_rust_semantic
                break
    if rust_semantic != dad_semantic:
        candidate = normalize_lexical_scope_declarations(rust_normalized)
        candidate_semantic = semantic_source(candidate, dad_source=False)
        if candidate_semantic == dad_semantic:
            rust_semantic = candidate_semantic
    if rust_semantic != dad_semantic:
        for candidate in lexical_scope_declaration_candidates(rust_normalized):
            candidate_semantic = semantic_source(candidate, dad_source=False)
            if candidate_semantic == dad_semantic:
                rust_semantic = candidate_semantic
                break
    if rust_semantic == dad_semantic:
        return None
    return name, dad_source, rust_source, dad_semantic, rust_semantic


def compare(
    dex_path: Path,
    executable: Path,
    diff_dir: Path | None,
    max_diff_print: int,
    class_name: str | None,
    threads: int,
    dad_cache_dir: Path | None,
    refresh_dad_cache: bool,
    diff_workers: int,
) -> tuple[int, int, float, float, list[float]]:
    dex_bytes = dex_path.read_bytes()
    dad_output, oracles, dad_seconds = decompile_dad_cached(
        dex_bytes,
        dad_cache_dir,
        refresh_dad_cache,
        dex_path.name,
    )
    filled_string_ranges = oracles["filled_string_ranges"]
    boolean_array_loads = oracles["boolean_array_loads"]
    object_array_loads = oracles["object_array_loads"]
    primitive_array_loads = oracles["primitive_array_loads"]
    inline_consts = oracles["inline_consts"]
    split_inline_consts = oracles["split_inline_consts"]
    catch_types = oracles["catch_types"]
    dad_type_corrections = oracles["dad_type_corrections"]
    rust, first_rust_seconds = decompile_rust(
        executable,
        dex_path,
        class_name,
        threads,
    )
    rust_times = [first_rust_seconds]
    for _ in range(1, RUST_TIMING_REPETITIONS):
        _, seconds = decompile_rust(executable, dex_path, class_name, threads)
        rust_times.append(seconds)
    rust_seconds = sum(rust_times) / len(rust_times)
    dad_names = set(dad_output)
    names = [class_name] if class_name else sorted(dad_names | set(rust))
    mismatches = 0
    print(
        f"SEMANTIC_DIFF {dex_path} runs=1 "
        "dad_references=1",
        flush=True,
    )
    tasks = (
        (
            name,
            dad_output.get(name),
            rust.get(name),
            filled_string_ranges.get(name, []),
            boolean_array_loads.get(name, []),
            object_array_loads.get(name, []),
            primitive_array_loads.get(name, []),
            inline_consts.get(name, []),
            split_inline_consts.get(name, []),
            catch_types.get(name, []),
            dad_type_corrections,
        )
        for name in names
    )
    if diff_workers == 1 or len(names) <= 1:
        results = map(compare_class, tasks)
        mismatch_details = [result for result in results if result is not None]
    else:
        with concurrent.futures.ProcessPoolExecutor(max_workers=diff_workers) as executor:
            results = executor.map(compare_class, tasks, chunksize=16)
            mismatch_details = [result for result in results if result is not None]
    mismatches = len(mismatch_details)
    for index, (name, dad_source, rust_source, dad_semantic, rust_semantic) in enumerate(
        mismatch_details, 1
    ):
        if index <= max_diff_print:
            print(
                f"DIFF {dex_path} {name} "
                f"dad={len(dad_source) if dad_source is not None else -1} "
                f"rust={len(rust_source) if rust_source is not None else -1}",
                flush=True,
            )
        if diff_dir is not None:
            write_difference(
                diff_dir,
                dex_path,
                name,
                dad_source or "",
                rust_source or "",
                dad_semantic,
                rust_semantic,
            )
    print(
        f"RUST_TIMES {dex_path} runs={len(rust_times)} "
        f"values={' '.join(f'{seconds:.6f}' for seconds in rust_times)} "
        f"average={rust_seconds:.6f}s",
        flush=True,
    )
    print(
        f"DEX {dex_path} classes={len(names)} mismatches={mismatches} "
        f"dad={dad_seconds:.6f}s rust_average={rust_seconds:.6f}s",
        flush=True,
    )
    return len(names), mismatches, dad_seconds, rust_seconds, rust_times


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Offline quality gate using frozen Androguard DAD pickle caches"
    )
    parser.add_argument("--phase", type=int, choices=(1, 2), default=2)
    parser.add_argument("--rust-bin", type=Path)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--max-diff-print", type=int, default=20)
    parser.add_argument(
        "--diff-workers",
        type=int,
        default=os.cpu_count() or 1,
        help="parallel class-comparison processes (default: logical CPU count)",
    )
    args = parser.parse_args()
    if args.diff_workers < 1:
        parser.error("--diff-workers must be at least 1")
    validate_normalizers()

    executable = args.rust_bin
    if executable is None:
        executable = ROOT / "target" / "release" / (
            "radguard.exe" if os.name == "nt" else "radguard"
        )
    if not args.no_build:
        executable = build_rust()
    if not executable.is_file():
        parser.error(f"RadGuard executable does not exist: {executable}")

    phases = (1,) if args.phase == 1 else (1, 2)
    ceilings = {1: 0, 2: 70}
    paths = {
        1: ROOT / "tests" / "fixtures" / "sample.dex",
        2: ROOT / "tests" / "fixtures" / "classes5.dex",
    }
    cache_dir = ROOT / "tests" / "fixtures" / "dad_reference_cache"
    for phase in phases:
        dex_path = paths[phase]
        if not dex_path.is_file():
            parser.error(f"required DEX fixture is missing: {dex_path}")
        classes, mismatches, _, rust_seconds, _ = compare(
            dex_path,
            executable,
            None,
            args.max_diff_print,
            None,
            1,
            cache_dir,
            False,
            args.diff_workers,
        )
        ceiling = ceilings[phase]
        status = "PASS" if mismatches <= ceiling else "FAIL"
        print(
            f"QUALITY_GATE phase={phase} classes={classes} mismatches={mismatches} "
            f"ceiling={ceiling} rust_seconds={rust_seconds:.6f} status={status}",
            flush=True,
        )
        if mismatches > ceiling:
            if phase == 1:
                print("PHASE2_BLOCKED phase1 requirements not met", flush=True)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
