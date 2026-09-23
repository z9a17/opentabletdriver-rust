using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json.Nodes;

namespace OtdUpstreamBench;

/// <summary>
/// The Rust harness's paced replay with upstream's scheduling: a time-critical
/// device thread sets an event once per interval, standing in for the HID
/// class driver completing a read, and a reader thread at AboveNormal in a
/// High priority class process (the daemon's settings, effective priority 14)
/// wakes, parses and runs the osu! profile's pipeline. Wake delay, pipeline
/// work and the SendInput call are timed separately, and so is each report's
/// whole time from the signal to the output call.
/// </summary>
sealed unsafe class Replay(Workload workload, string radialFollowPath, string radialFollowType, double seconds, bool sendInput, double nsPerTick)
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern nint CreateEventW(nint attributes, int manualReset, int initialState, nint name);

    [DllImport("kernel32.dll")]
    static extern int SetEvent(nint handle);

    [DllImport("kernel32.dll")]
    static extern uint WaitForSingleObject(nint handle, uint milliseconds);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern nint CreateWaitableTimerExW(nint attributes, nint name, uint flags, uint access);

    [DllImport("kernel32.dll")]
    static extern int SetWaitableTimer(nint timer, in long dueTime, int period, nint routine, nint argument, int resume);

    [DllImport("kernel32.dll")]
    static extern int CloseHandle(nint handle);

    [DllImport("kernel32.dll")]
    static extern nint GetCurrentThread();

    [DllImport("kernel32.dll")]
    static extern int SetThreadPriority(nint thread, int priority);

    const uint HighResolution = 0x2, TimerAllAccess = 0x1F0003, Infinite = 0xFFFFFFFF;
    const int TimeCritical = 15;

    long signaled, sequence;
    volatile bool done;

    void Device(nint readEvent, long reports, TimeSpan interval)
    {
        SetThreadPriority(GetCurrentThread(), TimeCritical);
        nint timer = CreateWaitableTimerExW(0, 0, HighResolution, TimerAllAccess);
        if (timer == 0)
            throw new InvalidOperationException($"CreateWaitableTimerExW failed: {Marshal.GetLastWin32Error()}");
        long start = Stopwatch.GetTimestamp() + Stopwatch.Frequency / 20;
        for (long index = 0; index < reports; index++)
        {
            long due = start + (long)(interval.TotalSeconds * Stopwatch.Frequency * index);
            long now = Stopwatch.GetTimestamp();
            if (due > now)
            {
                // Relative due time in 100 ns units.
                long wait = -(long)((due - now) * 1e7 / Stopwatch.Frequency);
                SetWaitableTimer(timer, in wait, 0, 0, 0, 0);
                WaitForSingleObject(timer, Infinite);
            }
            Volatile.Write(ref signaled, (long)Clock.Start());
            Volatile.Write(ref sequence, index + 1);
            SetEvent(readEvent);
        }
        CloseHandle(timer);
    }

    public JsonObject Run()
    {
        double rateHz = workload.Json["trace"]!["rate_hz"]!.GetValue<double>();
        long reports = (long)Math.Round(rateHz * seconds);
        if (reports < 200)
            throw new ArgumentException("replay needs at least 200 reports");
        var interval = TimeSpan.FromSeconds(1 / rateHz);
        nint readEvent = CreateEventW(0, 0, 0, 0);
        if (readEvent == 0)
            throw new InvalidOperationException($"CreateEventW failed: {Marshal.GetLastWin32Error()}");

        var windowsPointer = sendInput ? new MoveOnlyWindowsPointer() : null;
        var nullPointer = new NullPointer();
        var filter = Pipelines.Filter(radialFollowPath, radialFollowType, workload.RadialFollow);
        var reader = windowsPointer != null
            ? Pipelines.Absolute(workload, windowsPointer, windowsPointer, filter)
            : Pipelines.Absolute(workload, nullPointer, nullPointer, filter);
        // Warm the pipeline up as the other cases do, off the clock.
        var warming = Stopwatch.StartNew();
        var warmPointer = new NullPointer();
        var warmReader = Pipelines.Absolute(workload, warmPointer, warmPointer, Pipelines.Filter(radialFollowPath, radialFollowType, workload.RadialFollow));
        for (int i = 0; warming.ElapsedMilliseconds < 1000; i++)
            warmReader.OnData(workload.Reports[i % workload.Reports.Length]);
        Thread.Sleep(250);

        var wake = new ulong[reports];
        var work = new ulong[reports];
        var output = new ulong[reports];
        var driver = new ulong[reports];
        int processed = 0;
        long coalesced = 0, bytes = 0;
        int[] collections = new int[3];

        var process = Process.GetCurrentProcess();
        var previousClass = process.PriorityClass;
        process.PriorityClass = ProcessPriorityClass.High;
        var readerThread = new Thread(() =>
        {
            long last = 0;
            int[] before = [GC.CollectionCount(0), GC.CollectionCount(1), GC.CollectionCount(2)];
            long allocated = GC.GetAllocatedBytesForCurrentThread();
            while (last < reports)
            {
                WaitForSingleObject(readEvent, 1000);
                ulong woke = Clock.Stop();
                long current = Volatile.Read(ref sequence);
                if (current == last)
                {
                    if (done)
                        break;
                    continue;
                }
                coalesced += current - last - 1;
                last = current;
                ulong wokeAfter = woke - (ulong)Volatile.Read(ref signaled);
                wake[processed] = wokeAfter;
                byte[] data = workload.Reports[(last - 1) % workload.Reports.Length];
                if (windowsPointer != null)
                    windowsPointer.LastFlushTicks = 0;
                ulong begin = Clock.Start();
                reader.OnData(data);
                ulong total = Clock.Stop() - begin;
                ulong sent = windowsPointer?.LastFlushTicks ?? 0;
                work[processed] = total - sent;
                output[processed] = sent;
                driver[processed] = wokeAfter + total - sent;
                processed++;
            }
            bytes = GC.GetAllocatedBytesForCurrentThread() - allocated;
            collections = [GC.CollectionCount(0) - before[0], GC.CollectionCount(1) - before[1], GC.CollectionCount(2) - before[2]];
        })
        { Priority = ThreadPriority.AboveNormal, Name = "Replay reader" };
        var deviceThread = new Thread(() =>
        {
            Device(readEvent, reports, interval);
            done = true;
        })
        { Name = "Replay device" };
        readerThread.Start();
        deviceThread.Start();
        deviceThread.Join();
        SetEvent(readEvent);
        readerThread.Join();
        process.PriorityClass = previousClass;
        CloseHandle(readEvent);

        return new JsonObject
        {
            ["rate_hz"] = rateHz,
            ["reports"] = reports,
            ["processed"] = processed,
            ["coalesced"] = coalesced,
            ["send_input"] = sendInput,
            ["wake_ns"] = Stats.Json(Stats.Of(wake, processed, nsPerTick)),
            ["pipeline_ns"] = Stats.Json(Stats.Of(work, processed, nsPerTick)),
            ["output_ns"] = Stats.Json(Stats.Of(output, processed, nsPerTick)),
            ["signal_to_output_ns"] = Stats.Json(Stats.Of(driver, processed, nsPerTick)),
            ["gc_bytes_per_report"] = Stats.Round1((double)bytes / Math.Max(processed, 1)),
            ["gc_collections"] = new JsonObject { ["gen0"] = collections[0], ["gen1"] = collections[1], ["gen2"] = collections[2] },
        };
    }
}
