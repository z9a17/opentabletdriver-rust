// Times OpenTabletDriver 0.6.7's own report path on the workload that the Rust
// harness (examples/bench) exports: the same reports, profile, method and
// JSON schema. See docs/PERFORMANCE.md. With --reference it instead fills the
// differential fixtures in tests/differential from the same pipeline.

using System.Diagnostics;
using System.Numerics;
using System.Runtime;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using OpenTabletDriver.Configurations.Parsers.Wacom.IntuosV2;
using OpenTabletDriver.Desktop.Interop.Input.Absolute;

namespace OtdUpstreamBench;

sealed class Options
{
    public string Workload = "";
    public string? RadialFollow, Out, Only;
    public List<string> Reference = [];
    public int Rounds = 7, WarmupMs = 1000;
    public bool SendInput;
    public double ReplaySeconds;

    public const string Usage = """
        Usage: OtdUpstreamBench --workload DIR/workload.json [options]

          --radialfollow DLL      unchanged RadialFollow.dll; adds the osu! profile cases
          --out FILE              write the JSON results to FILE instead of stdout
          --rounds N              timed passes per case (default 7)
          --warmup-ms N           warm-up per round before the timed pass (default 1000)
          --send-input            adds SendInput cases; moves the cursor, never clicks
          --replay-seconds N      paced replay length; 0 skips it (default 0)
          --only TEXT             runs only cases whose name contains TEXT

        Usage: OtdUpstreamBench --reference FIXTURE.json [--reference ...] [--radialfollow DLL]

          fills the expected outputs of differential fixtures (tests/differential)
          from OpenTabletDriver's own pipeline; OTD_UPSTREAM_COMMIT names the
          pinned revision
        """;

    public static Options Parse(string[] args)
    {
        var options = new Options();
        for (int i = 0; i < args.Length; i++)
        {
            string Value() => ++i < args.Length ? args[i] : throw new ArgumentException($"{args[i - 1]} needs a value");
            switch (args[i])
            {
                case "--workload": options.Workload = Value(); break;
                case "--radialfollow": options.RadialFollow = Value(); break;
                case "--out": options.Out = Value(); break;
                case "--rounds": options.Rounds = int.Parse(Value()); break;
                case "--warmup-ms": options.WarmupMs = int.Parse(Value()); break;
                case "--send-input": options.SendInput = true; break;
                case "--replay-seconds": options.ReplaySeconds = double.Parse(Value(), System.Globalization.CultureInfo.InvariantCulture); break;
                case "--only": options.Only = Value(); break;
                case "--reference": options.Reference.Add(Value()); break;
                case "--help" or "-h": Console.WriteLine(Usage); Environment.Exit(0); break;
                default: throw new ArgumentException($"unknown option {args[i]}\n\n{Usage}");
            }
        }
        if (options.Workload.Length == 0 && options.Reference.Count == 0)
            throw new ArgumentException($"--workload or --reference is required\n\n{Usage}");
        if (options.Rounds < 1)
            throw new ArgumentException("use at least one round");
        return options;
    }
}

static class Stats
{
    static readonly string[] Fields = ["p50", "p95", "p99", "p999", "max", "mean"];

    public static double Round1(double value) => Math.Round(value * 10) / 10;

    /// <summary>Nearest-rank percentiles in nanoseconds; sorts the samples.</summary>
    public static double[] Of(ulong[] samples, int count, double nsPerTick)
    {
        Array.Sort(samples, 0, count);
        double Rank(double fraction) => samples[Math.Clamp((int)Math.Ceiling(count * fraction), 1, count) - 1] * nsPerTick;
        double mean = 0;
        for (int i = 0; i < count; i++)
            mean += samples[i];
        return [Rank(0.5), Rank(0.95), Rank(0.99), Rank(0.999), samples[count - 1] * nsPerTick, mean / count * nsPerTick];
    }

    public static JsonObject Json(double[] distribution)
    {
        var json = new JsonObject();
        for (int i = 0; i < Fields.Length; i++)
            json[Fields[i]] = Round1(distribution[i]);
        return json;
    }

    public static double Median(IEnumerable<double> values)
    {
        var sorted = values.Order().ToArray();
        int middle = sorted.Length / 2;
        return sorted.Length % 2 == 0 ? (sorted[middle - 1] + sorted[middle]) / 2 : sorted[middle];
    }

    public static JsonObject Across(IReadOnlyList<double[]> rounds)
    {
        var json = new JsonObject();
        for (int i = 0; i < Fields.Length; i++)
        {
            var values = rounds.Select(r => r[i]).ToArray();
            double median = Median(values), low = values.Min(), high = values.Max();
            json[Fields[i]] = new JsonObject
            {
                ["median"] = Round1(median),
                ["min"] = Round1(low),
                ["max"] = Round1(high),
                ["spread_pct"] = Round1(median > 0 ? (high - low) / median * 100 : 0),
            };
        }
        return json;
    }
}

sealed record Round(double[] PerReport, double ThreadNs, double WallSeconds, long Bytes, int[] Collections);

static unsafe class Program
{
    [DllImport("user32.dll")] static extern int GetCursorPos(out Point point);
    [DllImport("user32.dll")] static extern int SetCursorPos(int x, int y);
    struct Point { public int X, Y; }

    /// <summary>Keeps results alive so the JIT cannot drop the work.</summary>
    public static object? Sink;

    static int Main(string[] args)
    {
        try
        {
            var options = Options.Parse(args);
            if (options.Reference.Count > 0)
            {
                string? radialFollow = options.RadialFollow == null ? null : Path.GetFullPath(options.RadialFollow);
                foreach (string fixture in options.Reference)
                    Reference.Fill(fixture, radialFollow);
                return 0;
            }
            Run(options);
            return 0;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine(error);
            return 1;
        }
    }

    static Round MeasureRound(Action<byte[]> process, byte[][] reports, int warmupMs, double nsPerTick, double nsPerCycle, ulong[] samples)
    {
        int count = reports.Length;
        long sequence = 0;
        var warming = Stopwatch.StartNew();
        while (warming.ElapsedMilliseconds < warmupMs || sequence < count)
            for (int i = 0; i < 1000; i++)
                process(reports[sequence++ % count]);
        // Let tiered compilation finish, then warm up briefly again.
        Thread.Sleep(250);
        for (int i = 0; i < 2000; i++)
            process(reports[sequence++ % count]);

        int[] collections = [GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2)];
        long bytes = GC.GetAllocatedBytesForCurrentThread();
        ulong cycles = Clock.ThreadCycles();
        long wall = Stopwatch.GetTimestamp();
        for (int i = 0; i < count; i++)
        {
            byte[] report = reports[i];
            ulong begin = Clock.Start();
            process(report);
            samples[i] = Clock.Stop() - begin;
        }
        double wallSeconds = Stopwatch.GetElapsedTime(wall).TotalSeconds;
        cycles = Clock.ThreadCycles() - cycles;
        bytes = GC.GetAllocatedBytesForCurrentThread() - bytes;
        collections = [GC.CollectionCount(0) - collections[0], GC.CollectionCount(1) - collections[1], GC.CollectionCount(2) - collections[2]];
        return new Round(Stats.Of(samples, count, nsPerTick), cycles * nsPerCycle / count, wallSeconds, bytes, collections);
    }

    static void Run(Options options)
    {
        var workload = Workload.Load(options.Workload);
        var reports = workload.Reports;
        Console.Error.WriteLine("calibrating the time-stamp counter");
        double tscHz = Clock.TscHz(), threadCycleHz = Clock.ThreadCycleHz();
        double nsPerTick = 1e9 / tscHz, nsPerCycle = 1e9 / threadCycleHz;
        var samples = new ulong[reports.Length];
        var cases = new JsonArray();
        const string radialFollowType = "RadialFollow.RadialFollowSmoothingTabletSpace";

        void Case(string name, string description, Func<Action<byte[]>> make)
        {
            if (options.Only != null && !name.Contains(options.Only))
                return;
            Console.Error.WriteLine($"case {name}");
            var rounds = new List<Round>();
            var setupMs = new JsonArray();
            var firstUs = new JsonArray();
            for (int round = 0; round < options.Rounds; round++)
            {
                long created = Stopwatch.GetTimestamp();
                var process = make();
                setupMs.Add(Stats.Round1(Stopwatch.GetElapsedTime(created).TotalMilliseconds));
                ulong begin = Clock.Start();
                process(reports[0]);
                firstUs.Add(Stats.Round1((Clock.Stop() - begin) * nsPerTick / 1e3));
                rounds.Add(MeasureRound(process, reports, options.WarmupMs, nsPerTick, nsPerCycle, samples));
            }
            long bytes = rounds.Max(r => r.Bytes);
            cases.Add(new JsonObject
            {
                ["name"] = name,
                ["description"] = description,
                ["reports"] = reports.Length,
                ["rounds"] = rounds.Count,
                ["per_report_ns"] = Stats.Across(rounds.Select(r => r.PerReport).ToList()),
                ["thread_cpu_ns_per_report"] = Stats.Round1(Stats.Median(rounds.Select(r => r.ThreadNs))),
                ["throughput_per_s"] = Math.Round(Stats.Median(rounds.Select(r => reports.Length / r.WallSeconds))),
                ["gc_bytes_per_report"] = Stats.Round1((double)bytes / reports.Length),
                ["gc_collections"] = new JsonObject
                {
                    ["gen0"] = rounds.Max(r => r.Collections[0]),
                    ["gen1"] = rounds.Max(r => r.Collections[1]),
                    ["gen2"] = rounds.Max(r => r.Collections[2]),
                },
                ["setup_ms"] = setupMs,
                ["first_report_us"] = firstUs,
            });
        }

        Case("empty", "the harness alone: timing and one delegate call per report", () => data => { });
        Case("parse", "IntuosV2ReportParser.Parse on each 192-byte report", () =>
        {
            var parser = new IntuosV2ReportParser();
            return data => Sink = parser.Parse(data);
        });
        Case("absolute", "the daemon's report path: parse, report event, tree lock, absolute mode and the tip binding; the pointer keeps the position", () =>
        {
            var pointer = new NullPointer();
            return Pipelines.Absolute(workload, pointer, pointer, null).OnData;
        });
        Case("relative", "as absolute, in relative mode at 10 counts/mm", () => Pipelines.Relative(workload, new NullPointer()).OnData);
        if (options.RadialFollow != null)
        {
            string path = Path.GetFullPath(options.RadialFollow);
            Case("absolute+managed_radial_follow", "as absolute, with the unchanged RadialFollow 0.3.0 tablet-space filter (the osu! profile)", () =>
            {
                var pointer = new NullPointer();
                return Pipelines.Absolute(workload, pointer, pointer, Pipelines.Filter(path, radialFollowType, workload.RadialFollow)).OnData;
            });
            Case("absolute+managed_radial_follow+read_buffer", "as the osu! profile case, with a new 192-byte buffer per report as the HID read returns", () =>
            {
                var pointer = new NullPointer();
                var reader = Pipelines.Absolute(workload, pointer, pointer, Pipelines.Filter(path, radialFollowType, workload.RadialFollow));
                return data =>
                {
                    var buffer = new byte[data.Length];
                    Buffer.BlockCopy(data, 0, buffer, 0, data.Length);
                    reader.OnData(buffer);
                };
            });
        }

        Point? cursor = null;
        if (options.SendInput && options.RadialFollow != null)
        {
            string path = Path.GetFullPath(options.RadialFollow);
            GetCursorPos(out var saved);
            cursor = saved;
            // The positions the osu! profile produces for the trace.
            var positions = new List<Vector2>();
            var recorder = new RecordingPointer(positions);
            var producer = Pipelines.Absolute(workload, recorder, recorder, Pipelines.Filter(path, radialFollowType, workload.RadialFollow));
            foreach (var report in reports)
                producer.OnData(report);
            Case("sendinput", "WindowsAbsolutePointer.SetPosition and Flush for each position the osu! profile produces: one SendInput call each", () =>
            {
                var pointer = new WindowsAbsolutePointer();
                int next = 0;
                return _ =>
                {
                    pointer.SetPosition(positions[next++ % positions.Count]);
                    pointer.Flush();
                };
            });
            Case("absolute+managed_radial_follow+sendinput", "the osu! profile's whole report path with SendInput, buttons dropped", () =>
            {
                var pointer = new MoveOnlyWindowsPointer();
                return Pipelines.Absolute(workload, pointer, pointer, Pipelines.Filter(path, radialFollowType, workload.RadialFollow)).OnData;
            });
        }

        JsonNode? replay = null;
        if (options.ReplaySeconds > 0)
        {
            if (options.RadialFollow == null)
                throw new ArgumentException("the replay runs the osu! profile and needs --radialfollow");
            if (options.SendInput && cursor == null)
            {
                GetCursorPos(out var saved);
                cursor = saved;
            }
            Console.Error.WriteLine($"paced replay for {options.ReplaySeconds} s");
            replay = new Replay(workload, Path.GetFullPath(options.RadialFollow), radialFollowType, options.ReplaySeconds, options.SendInput, nsPerTick).Run();
        }
        if (cursor is { } point)
            SetCursorPos(point.X, point.Y);

        var trace = workload.Json["trace"]!;
        var results = new JsonObject
        {
            ["schema"] = "otd-bench/1",
            ["harness"] = "upstream",
            ["driver"] = new JsonObject
            {
                ["name"] = "OpenTabletDriver",
                ["version"] = typeof(IntuosV2ReportParser).Assembly.GetName().Version?.ToString(3),
                ["commit"] = Environment.GetEnvironmentVariable("OTD_UPSTREAM_COMMIT"),
            },
            ["runtime"] = new JsonObject
            {
                ["framework"] = RuntimeInformation.FrameworkDescription,
                ["server_gc"] = GCSettings.IsServerGC,
                ["gc_latency_mode"] = GCSettings.LatencyMode.ToString(),
                ["tiered_pgo"] = AppContext.TryGetSwitch("System.Runtime.TieredPGO", out bool pgo) ? pgo : null,
            },
            ["machine"] = new JsonObject
            {
                ["cpu"] = Clock.CpuBrand(),
                ["logical_cpus"] = Environment.ProcessorCount,
                ["tsc_hz"] = Math.Round(tscHz),
                ["thread_cycle_hz"] = Math.Round(threadCycleHz),
                ["invariant_tsc"] = Clock.InvariantTsc(),
            },
            ["trace"] = trace.DeepClone(),
            ["method"] = new JsonObject
            {
                ["rounds"] = options.Rounds,
                ["warmup_ms"] = options.WarmupMs,
                ["timer"] = "rdtsc/rdtscp with lfence around each report, converted with the measured counter frequency",
                ["allocations"] = "managed bytes allocated on the measuring thread during the timed pass",
            },
            ["cases"] = cases,
            ["replay"] = replay,
        };
        string text = results.ToJsonString(new JsonSerializerOptions
        {
            WriteIndented = true,
            TypeInfoResolver = new System.Text.Json.Serialization.Metadata.DefaultJsonTypeInfoResolver(),
        });
        if (options.Out != null)
            File.WriteAllText(options.Out, text);
        else
            Console.WriteLine(text);
    }
}

/// <summary>Records every position the pipeline outputs.</summary>
sealed class RecordingPointer(List<Vector2> positions) : OpenTabletDriver.Plugin.Platform.Pointer.IAbsolutePointer,
    OpenTabletDriver.Plugin.Platform.Pointer.IMouseButtonHandler
{
    public void SetPosition(Vector2 pos) => positions.Add(pos);
    public void MouseDown(OpenTabletDriver.Plugin.Platform.Pointer.MouseButton button) { }
    public void MouseUp(OpenTabletDriver.Plugin.Platform.Pointer.MouseButton button) { }
}
