using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Runtime.Intrinsics.X86;

namespace OtdUpstreamBench;

/// <summary>
/// The same timestamps as the Rust harness: lfence and rdtsc before the timed
/// work, rdtscp and lfence after it. .NET has no rdtsc intrinsic, so two tiny
/// machine-code stubs are called through unmanaged function pointers without
/// a GC transition.
/// </summary>
static unsafe class Clock
{
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern nint VirtualAlloc(nint address, nuint size, uint type, uint protect);

    [DllImport("kernel32.dll", SetLastError = true)]
    static extern int VirtualProtect(nint address, nuint size, uint protect, out uint old);

    [DllImport("kernel32.dll")]
    static extern nint GetCurrentThread();

    [DllImport("kernel32.dll")]
    static extern int QueryThreadCycleTime(nint thread, out ulong cycles);

    public static readonly delegate* unmanaged[SuppressGCTransition]<ulong> Start;
    public static readonly delegate* unmanaged[SuppressGCTransition]<ulong> Stop;

    static Clock()
    {
        // lfence; rdtsc; lfence; shl rdx, 32; or rax, rdx; ret
        byte[] start = [0x0F, 0xAE, 0xE8, 0x0F, 0x31, 0x0F, 0xAE, 0xE8, 0x48, 0xC1, 0xE2, 0x20, 0x48, 0x09, 0xD0, 0xC3];
        // rdtscp; lfence; shl rdx, 32; or rax, rdx; ret (rcx is volatile)
        byte[] stop = [0x0F, 0x01, 0xF9, 0x0F, 0xAE, 0xE8, 0x48, 0xC1, 0xE2, 0x20, 0x48, 0x09, 0xD0, 0xC3];
        nint page = VirtualAlloc(0, 4096, 0x3000, 0x04);
        if (page == 0)
            throw new InvalidOperationException($"VirtualAlloc failed: {Marshal.GetLastWin32Error()}");
        Marshal.Copy(start, 0, page, start.Length);
        Marshal.Copy(stop, 0, page + 64, stop.Length);
        if (VirtualProtect(page, 4096, 0x20, out _) == 0)
            throw new InvalidOperationException($"VirtualProtect failed: {Marshal.GetLastWin32Error()}");
        Start = (delegate* unmanaged[SuppressGCTransition]<ulong>)page;
        Stop = (delegate* unmanaged[SuppressGCTransition]<ulong>)(page + 64);
    }

    public static ulong ThreadCycles()
    {
        QueryThreadCycleTime(GetCurrentThread(), out ulong cycles);
        return cycles;
    }

    static double Median(IEnumerable<double> values)
    {
        var sorted = values.Order().ToArray();
        return sorted[sorted.Length / 2];
    }

    /// <summary>Counter ticks per second: the median of five 100 ms comparisons.</summary>
    public static double TscHz() => Median(Enumerable.Range(0, 5).Select(_ =>
    {
        long instant = Stopwatch.GetTimestamp();
        ulong ticks = Start();
        Thread.Sleep(100);
        ulong elapsedTicks = Stop() - ticks;
        return elapsedTicks / Stopwatch.GetElapsedTime(instant).TotalSeconds;
    }));

    /// <summary>The rate of <see cref="ThreadCycles"/> while the thread runs.</summary>
    public static double ThreadCycleHz() => Median(Enumerable.Range(0, 5).Select(_ =>
    {
        long instant = Stopwatch.GetTimestamp();
        ulong cycles = ThreadCycles();
        while (Stopwatch.GetElapsedTime(instant).TotalMilliseconds < 50)
            Thread.SpinWait(10);
        return (ThreadCycles() - cycles) / Stopwatch.GetElapsedTime(instant).TotalSeconds;
    }));

    public static bool InvariantTsc() =>
        (uint)X86Base.CpuId(unchecked((int)0x80000000), 0).Eax >= 0x80000007
        && (X86Base.CpuId(unchecked((int)0x80000007), 0).Edx & (1 << 8)) != 0;

    public static string CpuBrand()
    {
        var bytes = new List<byte>();
        for (uint leaf = 0x80000002; leaf <= 0x80000004; leaf++)
        {
            var (a, b, c, d) = X86Base.CpuId(unchecked((int)leaf), 0);
            foreach (int value in new[] { a, b, c, d })
                bytes.AddRange(BitConverter.GetBytes(value));
        }
        return System.Text.Encoding.ASCII.GetString(bytes.ToArray()).Trim('\0', ' ');
    }
}
