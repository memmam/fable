#!/usr/bin/env python3
"""Interleaved A/B benchmark between two Fable checkouts.

Usage: bench/ab.py BASE_DIR HEAD_DIR [--micro-n N] [--macro-n N]
                   [--targets name,name,...] [--threshold PCT]

Each side is a full checkout with its own `target/release/fable[.exe]`
already built; every target runs against *its own* tree, so the two sides
stay fair even when bench/demo/test sources differ between the refs
(a binary is never asked to run the other ref's programs).

Method (bench/RESULTS.md): runs are interleaved A,B,A,B,... within one
batch so machine drift hits both sides equally; the minimum wall time per
side is the least-noise estimator; only the relative delta is meaningful.

Checksum enforcement — exactly what is checked:
  * per-side stability: every rep of a (target, side) pair, warm-up
    included, must produce byte-identical stdout; a mismatch is a hard
    failure naming the target and side (a bench that prints different
    checksums across reps did not do the same work each rep);
  * cross-side equality: when a target's sources are byte-identical
    between the two trees (the single .fable file for micros, the whole
    demo directory for macros), base stdout must equal head stdout —
    a wrong-answer "optimization" fails the run instead of winning it;
  * when the sources differ between the trees, only per-side stability
    applies (the two refs may legitimately print different checksums).
stdout is captured and compared as raw bytes (Fable prints \\n on every
platform), so the checks are immune to Windows text-encoding surprises;
decoding happens only in diagnostics, UTF-8 with replacement.

Targets are enumerated from the union of both trees: present in both is
a delta row; present only in base is reported "removed in head" and
skipped; present only in head is reported "added in head" and timed
head-only (no delta).

The exit code is 0 unless a target crashes or a checksum check fails;
judging deltas against the noise threshold is the caller's job (the
table marks |delta| >= the threshold, default 3%).
"""
import argparse
import os
import subprocess
import sys
import time

MICRO_DIR = "bench"
MACROS = [
    ("lisp", ["demos/lisp/main.fable"]),
    ("checkers", ["demos/checkers/main.fable"]),
    ("wfc", ["demos/wfc/main.fable"]),
    ("sudoku", ["demos/sudoku/main.fable"]),
    ("reversi", ["demos/reversi/main.fable"]),
    ("spectra", ["demos/spectra/main.fable"]),
    ("png", ["demos/png/main.fable"]),
]
# The spec suite is deliberately not a target: its sources move with each
# ref, so cross-ref wall times compare different suites, not the binary.


def binary(root):
    # Absolute, because the subprocess runs with cwd=root: POSIX exec
    # resolves a relative program path in the *child* cwd (root/root/...),
    # while Windows resolves it in the parent's — absolute is the only
    # spelling that means the same binary on both.
    exe = os.path.join(os.path.abspath(root), "target", "release", "fable")
    if os.name == "nt" or not os.path.exists(exe):
        win = exe + ".exe"
        if os.path.exists(win):
            return win
    return exe


def targets_for(base, head, filter_names):
    # Union of both trees' bench/ dirs, so an added or removed bench is
    # reported instead of silently following one side's listing.
    names = set()
    for root in (base, head):
        micro = os.path.join(root, MICRO_DIR)
        if os.path.isdir(micro):
            for f in os.listdir(micro):
                if f.endswith(".fable"):
                    names.add(f[: -len(".fable")])
    out = [
        (name, [os.path.join(MICRO_DIR, name + ".fable")], "micro")
        for name in sorted(names)
    ]
    for name, args in MACROS:
        out.append((name, args, "macro"))
    if filter_names:
        keep = set(filter_names)
        out = [t for t in out if t[0] in keep]
    return out


def present(root, args):
    return all(os.path.exists(os.path.join(root, a)) for a in args)


def file_bytes(path):
    with open(path, "rb") as f:
        return f.read()


def sources_identical(base, head, args, kind):
    """True iff the target's sources are byte-identical between the trees.

    Micro scope is the single bench file; macro scope is the entry file's
    whole demo directory (a demo is a multi-file program plus data files —
    the entry alone does not determine its output).
    """
    if kind == "macro":
        for d in {os.path.dirname(a) for a in args}:
            bd, hd = os.path.join(base, d), os.path.join(head, d)
            brels, hrels = (
                {
                    os.path.relpath(os.path.join(dp, f), root)
                    for dp, _, fs in os.walk(root)
                    for f in fs
                }
                for root in (bd, hd)
            )
            if brels != hrels:
                return False
            for rel in brels:
                if file_bytes(os.path.join(bd, rel)) != file_bytes(
                    os.path.join(hd, rel)
                ):
                    return False
        return True
    return all(
        file_bytes(os.path.join(base, a)) == file_bytes(os.path.join(head, a))
        for a in args
    )


def run_once(root, args):
    t0 = time.perf_counter()
    proc = subprocess.run(
        [binary(root)] + args,
        cwd=root,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=True,
    )
    return time.perf_counter() - t0, proc.stdout


def preview(out):
    s = out.decode("utf-8", errors="replace").strip()
    return s if len(s) <= 120 else s[:117] + "..."


def check_stable(name, side, outs):
    for i, out in enumerate(outs[1:], start=1):
        if out != outs[0]:
            sys.exit(
                f"{name} [{side}]: stdout varies across reps "
                f"(rep 0: {preview(outs[0])!r} vs rep {i}: {preview(out)!r}) "
                f"— the bench is nondeterministic; every rep of one side "
                f"must print identical output"
            )


def timed_side(root, args, n):
    """One unmeasured warm-up, then n measured reps; returns
    (min seconds, stdout bytes) after enforcing rep-to-rep stability
    at the caller."""
    outs = [run_once(root, args)[1]]
    times = []
    for _ in range(n):
        t, out = run_once(root, args)
        times.append(t)
        outs.append(out)
    return min(times), outs


def main():
    # Windows Python pipes stdout/stderr as cp1252, which cannot encode
    # the ⚠ marker; the table is UTF-8 markdown wherever it lands.
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("base")
    ap.add_argument("head")
    ap.add_argument("--micro-n", type=int, default=5)
    ap.add_argument("--macro-n", type=int, default=3)
    ap.add_argument("--targets", default="")
    ap.add_argument("--threshold", type=float, default=3.0)
    opts = ap.parse_args()

    for side in ("base", "head"):
        exe = binary(getattr(opts, side))
        if not os.path.exists(exe):
            sys.exit(f"{side} binary missing: {exe} (build both sides first)")

    filt = [t for t in opts.targets.split(",") if t]
    rows = []  # (name, base_s, head_s, delta_pct, note)
    for name, args, kind in targets_for(opts.base, opts.head, filt):
        n = opts.micro_n if kind == "micro" else opts.macro_n
        if n <= 0:
            continue
        in_base, in_head = present(opts.base, args), present(opts.head, args)
        # Presence is checked against the source files, never inferred
        # from a run failure: a failed run is a real failure and must
        # fail the job.
        if not in_base and not in_head:
            rows.append((name, None, None, None, "missing on both refs"))
            continue
        if in_base and not in_head:
            print(f"{name}: removed in head — skipped", file=sys.stderr)
            rows.append((name, None, None, None, "removed in head (skipped)"))
            continue
        try:
            if in_head and not in_base:
                print(
                    f"{name}: added in head — timed head-only, no delta",
                    file=sys.stderr,
                )
                h, houts = timed_side(opts.head, args, n)
                check_stable(name, "head", houts)
                rows.append((name, None, h, None, "added in head (no delta)"))
                continue
            # One unmeasured warm-up per side, then strict interleaving.
            bouts = [run_once(opts.base, args)[1]]
            houts = [run_once(opts.head, args)[1]]
            bs, hs = [], []
            for _ in range(n):
                t, out = run_once(opts.base, args)
                bs.append(t)
                bouts.append(out)
                t, out = run_once(opts.head, args)
                hs.append(t)
                houts.append(out)
        except subprocess.CalledProcessError:
            print(f"{name}: FAILED to run on one side", file=sys.stderr)
            sys.exit(1)
        check_stable(name, "base", bouts)
        check_stable(name, "head", houts)
        if sources_identical(opts.base, opts.head, args, kind):
            if bouts[0] != houts[0]:
                sys.exit(
                    f"{name}: checksum mismatch — sources are byte-identical "
                    f"between the trees but stdout differs "
                    f"(base: {preview(bouts[0])!r}, head: {preview(houts[0])!r})"
                    f" — a wrong-answer optimization fails the run"
                )
        b, h = min(bs), min(hs)
        rows.append((name, b, h, 100.0 * (h - b) / b, None))

    print(f"| target | base (s) | head (s) | delta |")
    print(f"|--------|---------:|---------:|------:|")
    for name, b, h, d, note in rows:
        if d is None:
            bcol = "—" if b is None else f"{b:.4f}"
            hcol = "—" if h is None else f"{h:.4f}"
            print(f"| {name} | {bcol} | {hcol} | {note} |")
            continue
        mark = " ⚠" if abs(d) >= opts.threshold else ""
        print(f"| {name} | {b:.4f} | {h:.4f} | {d:+.1f}%{mark} |")
    print()
    print(
        f"best-of interleaved (micro n={opts.micro_n}, macro n={opts.macro_n}); "
        f"⚠ marks |delta| >= {opts.threshold}% (the noise floor on shared "
        f"runners — judge marked rows, ignore the rest). stdout enforced: "
        f"stable per side; equal across sides when sources are identical."
    )


if __name__ == "__main__":
    main()
