#!/usr/bin/env python3
"""Sample bread daemon/window resource use over a time window.

Metrics: per-process average/peak CPU, RSS/PSS, context switches, optional
RAPL package energy and battery power.  Scenarios are driven with --during.

Examples:
    python3 scripts/resource-smoke.py --duration 30
    python3 scripts/resource-smoke.py --duration 20 \
        --during 'for i in $(seq 1 20); do notify-send "test $i" body; done'
    python3 scripts/resource-smoke.py --duration 30 --json > resource.json
    python3 scripts/resource-smoke.py --duration 30 --max-avg-cpu 0.5
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

CLK_TCK = os.sysconf("SC_CLK_TCK")
PAGE_SIZE = os.sysconf("SC_PAGE_SIZE")
PROC = Path("/proc")
RAPL = Path("/sys/class/powercap")
POWER_SUPPLY = Path("/sys/class/power_supply")


def read_text(path: Path) -> str | None:
    try:
        return path.read_text()
    except OSError:
        return None


def discover() -> dict[int, str]:
    """Return {pid: 'daemon'|'window'} for live bread processes."""
    found: dict[int, str] = {}
    try:
        entries = list(PROC.iterdir())
    except OSError:
        return found
    for entry in entries:
        if not entry.name.isdigit():
            continue
        raw = read_text(entry / "cmdline")
        if not raw:
            continue
        args = [part for part in raw.split("\0") if part]
        if not args or Path(args[0]).name != "bread":
            continue
        if "__window" in args:
            kind = "window"
        elif len(args) == 1 or (len(args) > 1 and args[1] in ("daemon", "__daemon")):
            kind = "daemon"
        else:
            continue
        found[int(entry.name)] = kind
    return found


def read_cpu_seconds(pid: int) -> float | None:
    raw = read_text(PROC / str(pid) / "stat")
    if raw is None:
        return None
    try:
        fields = raw.rsplit(") ", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / CLK_TCK
    except (IndexError, ValueError):
        return None


def read_status(pid: int) -> dict[str, int]:
    raw = read_text(PROC / str(pid) / "status")
    if raw is None:
        return {}
    values: dict[str, int] = {}
    for line in raw.splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"):
            try:
                values[key] = int(value.strip().split()[0])
            except (IndexError, ValueError):
                pass
    return values


def read_pss_kb(pid: int) -> int:
    raw = read_text(PROC / str(pid) / "smaps_rollup")
    if raw is None:
        return 0
    for line in raw.splitlines():
        if line.startswith("Pss:"):
            try:
                return int(line.split()[1])
            except (IndexError, ValueError):
                return 0
    return 0


def snapshot(pid: int) -> dict[str, float] | None:
    cpu = read_cpu_seconds(pid)
    if cpu is None:
        return None
    status = read_status(pid)
    ctxt = status.get("voluntary_ctxt_switches", 0) + status.get(
        "nonvoluntary_ctxt_switches", 0
    )
    return {
        "cpu": cpu,
        "rss_kb": status.get("VmRSS", 0),
        "pss_kb": read_pss_kb(pid),
        "ctxt": ctxt,
    }


def read_energy_uj() -> float | None:
    """Sum package-level RAPL energy counters; None when unreadable."""
    total = 0.0
    found = False
    try:
        dirs = list(RAPL.iterdir())
    except OSError:
        return None
    for entry in dirs:
        if not re.fullmatch(r"intel-rapl:\d+", entry.name):
            continue
        raw = read_text(entry / "energy_uj")
        if raw is None:
            continue
        try:
            total += float(raw.strip())
            found = True
        except ValueError:
            pass
    return total if found else None


def read_battery_power_w() -> float | None:
    total = 0.0
    found = False
    try:
        entries = list(POWER_SUPPLY.iterdir())
    except OSError:
        return None
    for entry in entries:
        if not entry.name.startswith("BAT"):
            continue
        raw = read_text(entry / "power_now")
        if raw is None:
            continue
        try:
            total += float(raw.strip()) / 1_000_000.0
            found = True
        except ValueError:
            pass
    return total if found else None


def read_cpu_pressure() -> dict[str, float]:
    raw = read_text(Path("/proc/pressure/cpu")) or ""
    values: dict[str, float] = {}
    for line in raw.splitlines():
        parts = line.split()
        if not parts:
            continue
        for part in parts[1:]:
            key, _, value = part.partition("=")
            if key in ("avg10", "avg60", "avg300"):
                try:
                    values[f"{parts[0]}_{key}"] = float(value)
                except ValueError:
                    pass
    return values


def mb(kb: float) -> float:
    return kb / 1024.0


def run(duration: float, interval: float, during: str | None) -> dict:
    procs = discover()
    if not procs:
        raise SystemExit("resource-smoke: no bread daemon/window process found")

    state: dict[int, dict] = {}
    prev: dict[int, dict] = {}
    energy0 = read_energy_uj()
    battery_samples: list[float] = []
    peak_total_cpu = 0.0
    peak_total_rss_kb = 0.0

    start = time.monotonic()
    prev_time = start
    for pid, kind in procs.items():
        sample = snapshot(pid)
        if sample is None:
            continue
        prev[pid] = sample
        state[pid] = {
            "kind": kind,
            "cpu_delta": 0.0,
            "ctxt_delta": 0,
            "peak_cpu": 0.0,
            "last": sample,
            "peak_rss_kb": sample["rss_kb"],
        }

    during_proc = None
    during_rc = None
    if during:
        during_proc = subprocess.Popen(
            during, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )

    while True:
        now = time.monotonic()
        remaining = duration - (now - start)
        if remaining <= 0:
            break
        time.sleep(min(interval, remaining))
        now = time.monotonic()
        dt = now - prev_time
        if dt <= 0:
            continue

        current = discover()
        interval_total_cpu = 0.0
        interval_total_rss_kb = 0.0
        for pid, kind in current.items():
            sample = snapshot(pid)
            if sample is None:
                continue
            if pid not in prev:
                prev[pid] = sample
                state[pid] = {
                    "kind": kind,
                    "cpu_delta": 0.0,
                    "ctxt_delta": 0,
                    "peak_cpu": 0.0,
                    "last": sample,
                    "peak_rss_kb": sample["rss_kb"],
                }
                continue

            dcpu = max(0.0, sample["cpu"] - prev[pid]["cpu"])
            dctxt = max(0, sample["ctxt"] - prev[pid]["ctxt"])
            interval_cpu = dcpu / dt * 100.0
            interval_total_cpu += interval_cpu
            interval_total_rss_kb += sample["rss_kb"]

            item = state[pid]
            item["cpu_delta"] += dcpu
            item["ctxt_delta"] += dctxt
            item["peak_cpu"] = max(item["peak_cpu"], interval_cpu)
            item["last"] = sample
            item["peak_rss_kb"] = max(item["peak_rss_kb"], sample["rss_kb"])
            prev[pid] = sample

        peak_total_cpu = max(peak_total_cpu, interval_total_cpu)
        peak_total_rss_kb = max(peak_total_rss_kb, interval_total_rss_kb)
        battery = read_battery_power_w()
        if battery is not None:
            battery_samples.append(battery)
        prev_time = now

    elapsed = max(time.monotonic() - start, 1e-9)

    if during_proc is not None:
        try:
            during_rc = during_proc.wait(timeout=2)
        except subprocess.TimeoutExpired:
            during_proc.terminate()
            try:
                during_rc = during_proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                during_proc.kill()
                during_rc = during_proc.wait()

    energy1 = read_energy_uj()
    energy_j = None
    if energy0 is not None and energy1 is not None and energy1 >= energy0:
        energy_j = energy1 - energy0

    rows = []
    total_cpu_delta = 0.0
    total_ctxt = 0
    total_rss_kb = 0
    total_pss_kb = 0
    for pid in sorted(state):
        item = state[pid]
        cpu_delta = item["cpu_delta"]
        total_cpu_delta += cpu_delta
        total_ctxt += item["ctxt_delta"]
        last = item["last"]
        total_rss_kb += last["rss_kb"]
        total_pss_kb += last["pss_kb"]
        rows.append(
            {
                "pid": pid,
                "kind": item["kind"],
                "avg_cpu_percent": cpu_delta / elapsed * 100.0,
                "peak_cpu_percent": item["peak_cpu"],
                "rss_mb": mb(last["rss_kb"]),
                "pss_mb": mb(last["pss_kb"]),
                "peak_rss_mb": mb(item["peak_rss_kb"]),
                "ctxt_per_s": item["ctxt_delta"] / elapsed,
            }
        )

    result = {
        "duration_s": elapsed,
        "processes": rows,
        "total": {
            "avg_cpu_percent": total_cpu_delta / elapsed * 100.0,
            "peak_cpu_percent": peak_total_cpu,
            "rss_mb": mb(total_rss_kb),
            "pss_mb": mb(total_pss_kb),
            "peak_rss_mb": mb(peak_total_rss_kb),
            "ctxt_per_s": total_ctxt / elapsed,
        },
        "energy_j": energy_j,
        "energy_w": (energy_j / elapsed) if energy_j is not None else None,
        "battery_w": (sum(battery_samples) / len(battery_samples))
        if battery_samples
        else None,
        "cpu_pressure": read_cpu_pressure(),
        "during": {"command": during, "returncode": during_rc} if during else None,
    }
    return result


def print_human(result: dict) -> None:
    print(f"duration: {result['duration_s']:.1f} s")
    print(
        f"{'pid':>7} {'kind':<7} {'avg%':>7} {'peak%':>7} "
        f"{'rssMB':>7} {'pssMB':>7} {'peakRSS':>8} {'ctxt/s':>8}"
    )
    for row in result["processes"]:
        print(
            f"{row['pid']:>7} {row['kind']:<7} {row['avg_cpu_percent']:>7.2f} "
            f"{row['peak_cpu_percent']:>7.2f} {row['rss_mb']:>7.1f} "
            f"{row['pss_mb']:>7.1f} {row['peak_rss_mb']:>8.1f} "
            f"{row['ctxt_per_s']:>8.2f}"
        )
    total = result["total"]
    print(
        f"{'total':>7} {'':<7} {total['avg_cpu_percent']:>7.2f} "
        f"{total['peak_cpu_percent']:>7.2f} {total['rss_mb']:>7.1f} "
        f"{total['pss_mb']:>7.1f} {total['peak_rss_mb']:>8.1f} "
        f"{total['ctxt_per_s']:>8.2f}"
    )
    if result["energy_j"] is not None:
        print(f"energy: {result['energy_j']:.1f} J ({result['energy_w']:.2f} W, RAPL)")
    else:
        print("energy: N/A (RAPL unreadable; run with permission for package energy)")
    if result["battery_w"] is not None:
        print(f"battery: {result['battery_w']:.2f} W")
    else:
        print("battery: N/A")
    pressure = result["cpu_pressure"]
    if pressure:
        print(
            "cpu pressure: "
            + " ".join(f"{key}={value:.2f}" for key, value in sorted(pressure.items()))
        )
    during = result["during"]
    if during:
        print(f"during: {during['command']!r} (exit {during['returncode']})")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duration", type=float, default=30.0, help="seconds")
    parser.add_argument("--interval", type=float, default=0.25, help="sample interval")
    parser.add_argument("--during", help="shell command to run during the sample")
    parser.add_argument("--json", action="store_true", help="print JSON only")
    parser.add_argument("--max-avg-cpu", type=float, help="fail above total avg CPU %%")
    parser.add_argument("--max-peak-cpu", type=float, help="fail above total peak CPU %%")
    parser.add_argument("--max-rss-mb", type=float, help="fail above total peak RSS MB")
    parser.add_argument("--max-energy-j", type=float, help="fail above RAPL energy J")
    args = parser.parse_args()

    if args.duration <= 0 or args.interval <= 0:
        parser.error("--duration and --interval must be positive")

    result = run(args.duration, args.interval, args.during)
    failures = []
    total = result["total"]
    if args.max_avg_cpu is not None and total["avg_cpu_percent"] > args.max_avg_cpu:
        failures.append(
            f"avg CPU {total['avg_cpu_percent']:.2f}% > {args.max_avg_cpu:.2f}%"
        )
    if args.max_peak_cpu is not None and total["peak_cpu_percent"] > args.max_peak_cpu:
        failures.append(
            f"peak CPU {total['peak_cpu_percent']:.2f}% > {args.max_peak_cpu:.2f}%"
        )
    if args.max_rss_mb is not None and total["peak_rss_mb"] > args.max_rss_mb:
        failures.append(
            f"peak RSS {total['peak_rss_mb']:.1f}MB > {args.max_rss_mb:.1f}MB"
        )
    if args.max_energy_j is not None:
        if result["energy_j"] is None:
            failures.append("energy threshold set but RAPL is unreadable")
        elif result["energy_j"] > args.max_energy_j:
            failures.append(
                f"energy {result['energy_j']:.1f}J > {args.max_energy_j:.1f}J"
            )

    if args.json:
        result["failures"] = failures
        print(json.dumps(result, indent=2, sort_keys=True))
    else:
        print_human(result)
        if failures:
            for failure in failures:
                print(f"FAIL: {failure}", file=sys.stderr)
        else:
            print("ok: within thresholds")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
