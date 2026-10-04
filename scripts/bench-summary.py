"""Combines the JSON results that scripts/bench.ps1 writes into summary.md.

Usage: python scripts/bench-summary.py target/bench/<directory>

Each harness run reports, per case, percentiles that are already the median
over its timed rounds. This script takes the median over runs and shows the
range between runs, so noise is visible next to every number.
"""

import json
import statistics
import sys
from pathlib import Path

# (label, Rust case, upstream case); None where one side has no equivalent.
PAIRS = [
    ("Harness alone (included in every row)", "empty", "empty"),
    ("Decode one report", "parse", "parse"),
    ("Absolute mode, no filter", "absolute", "absolute"),
    ("Relative mode", "relative", "relative"),
    ("osu! profile: built-in Radial Follow / unchanged RadialFollow DLL", "absolute+radial_follow", "absolute+managed_radial_follow"),
    ("osu! profile, both with the unchanged RadialFollow DLL", "absolute+managed_radial_follow", "absolute+managed_radial_follow"),
    ("osu! profile with a new read buffer per report", None, "absolute+managed_radial_follow+read_buffer"),
    ("Native EMA filter DLL", "absolute+native_ema", None),
    ("Whole session loop, osu! profile, no HID read or SendInput", "session/absolute+radial_follow", None),
    ("Whole session loop, unchanged RadialFollow DLL, no HID read or SendInput", "session/absolute+managed_radial_follow", None),
    ("SendInput alone", "sendinput", "sendinput"),
    ("SendInput without cursor motion", "sendinput_still", None),
    ("osu! profile with SendInput", "absolute+radial_follow+sendinput", "absolute+managed_radial_follow+sendinput"),
]


def load(directory, prefix):
    return [json.loads(path.read_text(encoding="utf-8-sig")) for path in sorted(directory.glob(f"{prefix}-*.json"))]


def by_case(runs):
    table = {}
    for run in runs:
        for case in run["cases"]:
            table.setdefault(case["name"], []).append(case)
    return table


def span(values):
    """Median over runs, with the range when there is more than one run."""
    middle = statistics.median(values)
    if len(values) == 1 or max(values) == min(values):
        return fmt(middle)
    return f"{fmt(middle)} ({fmt(min(values))}–{fmt(max(values))})"


def fmt(ns):
    if ns >= 10_000:
        return f"{ns / 1000:.0f} µs"
    if ns >= 1_000:
        return f"{ns / 1000:.1f} µs"
    return f"{ns:.0f} ns"


def percentile(cases, name):
    return [case["per_report_ns"][name]["median"] for case in cases]


def cell(cases, name):
    return span(percentile(cases, name)) if cases else "—"


def memory(cases, harness):
    if not cases:
        return "—"
    if harness == "rust":
        return f"{max(case['allocations_per_report'] for case in cases):g} allocations"
    return f"{max(case['gc_bytes_per_report'] for case in cases):g} B"


def noise_passes(cases):
    """The widest p50 range between the timed passes of one run."""
    return f"{max(case['per_report_ns']['p50']['spread_pct'] for case in cases):.0f} %" if cases else "—"


def noise_runs(cases):
    """The p50 range between runs, as a percentage of their median."""
    if len(cases) < 2:
        return "—"
    values = percentile(cases, "p50")
    middle = statistics.median(values)
    return f"{(max(values) - min(values)) / middle * 100:.0f} %" if middle else "—"


def replay_table(runs, harness, field="replay"):
    replays = [run[field] for run in runs if run.get(field)]
    if not replays:
        return []
    # Separate rates, load levels and sinks instead of averaging unlike runs.
    groups = {}
    for replay in replays:
        configuration = (
            replay["rate_hz"],
            replay.get("controlled_cpu_load_threads", 0),
            replay["send_input"],
            replay.get("timing_endpoint", "legacy_excluding_output"),
        )
        groups.setdefault(configuration, []).append(replay)
    rows = []
    for (rate, load, send_input, endpoint), samples in groups.items():
        title = f"{harness}, {rate:g} Hz, {load} load threads"
        stages = [
            ("wake_ns", "Wake after the report is signaled"),
            ("pipeline_ns", "Decode, filter, map (output time excluded)"),
            ("signal_to_output_ns", "Signal to pipeline return, including output" if endpoint == "pipeline_return_including_output" else "Legacy total, output time excluded"),
            ("output_ns", "SendInput" if send_input else "Discard sink"),
        ]
        for key, label in stages:
            if key not in samples[0]:
                continue
            values = {p: [replay[key][p] for replay in samples] for p in ("p50", "p99", "p999", "max")}
            rows.append(f"| {title} | {label} | {span(values['p50'])} | {span(values['p99'])} | {span(values['p999'])} | {span(values['max'])} |")
        coalesced = sum(replay["coalesced"] for replay in samples)
        processed = sum(replay["processed"] for replay in samples)
        rows.append(f"| {title} | Reports processed / signals coalesced | {processed} / {coalesced} | | | |")
    return rows


def plan(text):
    """The power plan's name from powercfg's output."""
    start, end = text.rfind("("), text.rfind(")")
    return text[start + 1 : end] if 0 <= start < end else text


def main():
    directory = Path(sys.argv[1])
    rust_runs, upstream_runs = load(directory, "rust"), load(directory, "upstream")
    environment = json.loads((directory / "environment.json").read_text(encoding="utf-8-sig"))
    lines = [
        "# Performance measurements",
        "",
        f"- Rust commit `{environment['commit']}`{' (uncommitted changes)' if environment['dirty_worktree'] else ''}; OpenTabletDriver `{environment['upstream_revision']}`",
        f"- {environment['os']}; {environment['cpu']}, {environment['logical_cpus']} logical CPUs; power plan {plan(environment['power_plan'])}",
    ]
    if rust_runs or upstream_runs:
        lines += harness_sections(rust_runs, upstream_runs)
    lines += idle_section(directory)
    (directory / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")


def harness_sections(rust_runs, upstream_runs):
    rust, upstream = by_case(rust_runs), by_case(upstream_runs)
    sample = (rust_runs or upstream_runs)[0]
    lines = [
        (
            f"- Trace `{sample['trace']['generator']}`: {sample['trace']['reports']} reports, "
            f"FNV-1a `{sample['trace']['fnv1a64']}`; {len(rust_runs)} Rust runs, {len(upstream_runs)} upstream runs, "
            f"{sample['method']['rounds']} timed passes per case"
        ),
        "",
        "Per-report time is the median over runs of each run's median over timed passes; the range between runs follows in brackets.",
        "Every row includes the harness's own timing overhead, shown in the first row. Rust's allocation counter excludes the managed heap; zero Rust allocations does not mean a managed filter allocates nothing. Upstream's memory column is managed bytes allocated per report.",
        "",
        "| Case | Rust p50 | OTD p50 | Rust p99 | OTD p99 | Rust p99.9 | OTD p99.9 | Rust memory | OTD memory |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    if upstream_runs:
        lines.insert(0, f"- Upstream runtime {upstream_runs[0]['runtime']['framework']}")
    rows = [
        (label, rust.get(rust_name, []) if rust_name else [], upstream.get(upstream_name, []) if upstream_name else [])
        for label, rust_name, upstream_name in PAIRS
    ]
    rows = [(label, r, u) for label, r, u in rows if r or u]
    for label, r, u in rows:
        lines.append(
            f"| {label} | {cell(r, 'p50')} | {cell(u, 'p50')} | {cell(r, 'p99')} | {cell(u, 'p99')} | "
            f"{cell(r, 'p999')} | {cell(u, 'p999')} | {memory(r, 'rust')} | {memory(u, 'upstream')} |"
        )
    lines += [
        "",
        "## Mean, worst report and thread CPU time",
        "",
        "The worst report of a pass usually coincides with an interrupt or a context switch. Thread CPU time per report also covers the harness's untimed loop work.",
        "",
        "| Case | Rust mean | OTD mean | Rust max | OTD max | Rust CPU | OTD CPU |",
        "| --- | --- | --- | --- | --- | --- | --- |",
    ]
    for label, r, u in rows:
        rc = span([c["thread_cpu_ns_per_report"] for c in r]) if r else "—"
        uc = span([c["thread_cpu_ns_per_report"] for c in u]) if u else "—"
        lines.append(f"| {label} | {cell(r, 'mean')} | {cell(u, 'mean')} | {cell(r, 'max')} | {cell(u, 'max')} | {rc} | {uc} |")
    lines += [
        "",
        "## Noise",
        "",
        "How far the p50 moved: the widest range between the timed passes of one run, and the range between runs. Timestamps advance in steps of about 5 ns on this CPU, so short cases show large percentages.",
        "",
        "| Case | Rust passes | Rust runs | OTD passes | OTD runs |",
        "| --- | --- | --- | --- | --- |",
    ]
    for label, r, u in rows:
        lines.append(f"| {label} | {noise_passes(r)} | {noise_runs(r)} | {noise_passes(u)} | {noise_runs(u)} |")
    lines += ["", "## Startup", "", "First report after creating the case (µs) and the time to create it (ms), first timed pass of the first run:", ""]
    lines += ["| Harness | Case | First report | Setup |", "| --- | --- | --- | --- |"]
    for table, harness in ((rust, "Rust"), (upstream, "OTD")):
        for name, runs in table.items():
            if runs[0].get("first_report_us"):
                lines.append(f"| {harness} | {name} | {runs[0]['first_report_us'][0]:g} µs | {runs[0]['setup_ms'][0]:g} ms |")

    replay_rows = (
        replay_table(rust_runs, "Rust native")
        + replay_table(rust_runs, "Rust managed", "managed_replay")
        + replay_table(upstream_runs, "OTD managed")
    )
    if replay_rows:
        lines += [
            "",
            "## Paced replay",
            "",
            (
                "Rust's reader runs at time-critical priority; upstream's at AboveNormal in a High priority class "
                "process, as its daemon does. Rates, synthetic CPU load and output sinks are listed separately. "
                "The auto-reset event coalesces missed signals; it does not reproduce the HID queue. "
                "Compare drivers only at matching rates, sinks and load levels."
            ),
            "",
            "| Harness | Stage | p50 | p99 | p99.9 | max |",
            "| --- | --- | --- | --- | --- | --- |",
            *replay_rows,
        ]
    return lines


def idle_section(directory):
    lines = []
    idle_path = directory / "idle.json"
    if idle_path.exists():
        idle = json.loads(idle_path.read_text(encoding="utf-8-sig"))
        idle = idle if isinstance(idle, list) else [idle]
        lines += [
            "",
            "## Idle processes",
            "",
            "Tablet connected, pen away, after 10 s of warm-up.",
            "",
            "| Scenario | Process | CPU (% of one core) | Context switches/s | Working set | Private | Handles | Threads |",
            "| --- | --- | --- | --- | --- | --- | --- | --- |",
        ]
        for row in idle:
            lines.append(
                f"| {row['scenario']} | {row['process']} | {row['cpu_percent_of_one_core']:g} | {row['context_switches_per_s']:g} | "
                f"{row['working_set_mb']:g} MB | {row['private_mb']:g} MB | {row['handles']} | {row['threads']} |"
            )
    return lines


if __name__ == "__main__":
    main()
