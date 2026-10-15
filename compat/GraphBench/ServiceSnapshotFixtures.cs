using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using Newtonsoft.Json.Linq;
using OtdCompat;

// Pure mock ABI: no native owner, background monitor, plugin, device or output.
unsafe static class ServiceSnapshotProbe
{
    static ulong cacheVersion = 11, lastScope;
    static byte[] reply = [];
    static int polls, releases, ticketPolls, failPollOnCall, admissionError;
    static bool invalidRelease;

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Request(uint operation, ulong scope, byte* payload, uint length, ulong* ticket)
    {
        if (operation != 0 || payload != null || length != 0) return -3;
        if (admissionError != 0) return admissionError;
        lastScope = scope;
        *ticket = scope != 0 && scope == cacheVersion ? 0UL : 1UL;
        ticketPolls = 0;
        return 0;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static int Poll(ulong ticket, byte* output, uint capacity)
    {
        polls++; ticketPolls++;
        if (ticket != 1 || ticketPolls == failPollOnCall) return -3;
        if (output != null && capacity >= reply.Length) reply.AsSpan().CopyTo(new Span<byte>(output, reply.Length));
        return reply.Length;
    }

    [UnmanagedCallersOnly(CallConvs = [typeof(CallConvCdecl)])]
    static void Release(ulong ticket)
    {
        if (ticket != 1) { invalidRelease = true; return; }
        releases++;
    }

    static void Publish(ulong logicalVersion, ulong token)
    {
        cacheVersion = token;
        reply = Encoding.UTF8.GetBytes(new JObject {
            ["ok"] = true,
            ["result"] = new JObject { ["version"] = logicalVersion, ["cache_version"] = token }
        }.ToString(Newtonsoft.Json.Formatting.None));
    }

    static void ProviderNotificationChecks()
    {
        var previous = new JObject { ["version"] = 1, ["cache_version"] = 31,
            ["devices"] = new JArray("old-device"), ["tablets"] = new JArray("old-tablet") };
        var replacement = new JObject { ["version"] = 1, ["cache_version"] = 32,
            ["devices"] = new JArray("replacement-device"), ["tablets"] = new JArray("replacement-tablet") };
        // Exercise the production monitor gate: a replacement at the same
        // logical generation must reach its device/tablet difference dispatch.
        if (!ManagedProviders.SnapshotChanged(previous, replacement))
            throw new Exception("Provider notifications skipped a replacement owner at the same logical version.");
        if (ManagedProviders.SnapshotChanged(replacement, (JObject)replacement.DeepClone()))
            throw new Exception("An unchanged provider snapshot triggered notification comparison.");
        previous.Remove("cache_version"); replacement.Remove("cache_version");
        if (ManagedProviders.SnapshotChanged(previous, replacement))
            throw new Exception("Legacy provider snapshots changed without a logical version change.");
        replacement["version"] = 2;
        if (!ManagedProviders.SnapshotChanged(previous, replacement))
            throw new Exception("Legacy provider logical version changes stopped notifying.");
    }

    internal static void Run()
    {
        ProviderNotificationChecks();
        Type client = typeof(EntryPoints).Assembly.GetType("OtdCompat.ServiceClient", throwOnError: true)!;
        const BindingFlags flags = BindingFlags.Static | BindingFlags.NonPublic;
        var install = client.GetMethod("Install", flags)!.CreateDelegate<Action<ServiceCallbacks>>();
        install(new ServiceCallbacks { Version = 1, Size = (uint)sizeof(ServiceCallbacks), Request = &Request, Poll = &Poll, Release = &Release });
        var fetch = client.GetMethod("FetchSnapshot", flags)!.CreateDelegate<Func<JObject?, JObject>>();
        var refresh = client.GetMethod("RefreshSnapshot", flags, [typeof(bool)])!.CreateDelegate<Action<bool>>();
        var afterCommit = client.GetMethod("RefreshAfterCommit", flags)!.CreateDelegate<Action>();
        FieldInfo cached = client.GetField("snapshot", flags)!;

        Publish(1, 11);
        JObject first = fetch(null);
        if (lastScope != 0 || first.Value<ulong>("version") != 1 || polls != 2 || releases != 1)
            throw new Exception("Initial snapshot did not complete and release its full read.");
        int priorPolls = polls, priorReleases = releases;
        if (!ReferenceEquals(first, fetch(first)) || lastScope != 11 || polls != priorPolls || releases != priorReleases || invalidRelease)
            throw new Exception("Unchanged snapshot allocated/parses a completion or released ticket zero.");

        Publish(2, 12);
        JObject changed = fetch(first);
        if (ReferenceEquals(changed, first) || changed.Value<ulong>("version") != 2 || releases != priorReleases + 1)
            throw new Exception("Changed snapshot was not fetched.");
        Publish(1, 13); // Replacement owner resets its logical version, never the cache token.
        JObject replacement = fetch(changed);
        if (replacement.Value<ulong>("version") != 1 || replacement.Value<ulong>("cache_version") != 13)
            throw new Exception("Replacement owner was hidden by a reused logical generation.");

        cached.SetValue(null, replacement);
        refresh(false);
        if (!ReferenceEquals(cached.GetValue(null), replacement)) throw new Exception("Periodic unchanged read replaced the cached object.");
        afterCommit();
        if (lastScope != 0 || ReferenceEquals(cached.GetValue(null), replacement))
            throw new Exception("Post-commit refresh did not force a full snapshot.");

        for (int failAt = 1; failAt <= 2; failAt++) {
            failPollOnCall = failAt;
            int released = releases;
            try { fetch(null); throw new Exception("Expired completion was accepted."); }
            catch (InvalidOperationException) { }
            if (releases != released + 1) throw new Exception("Expired snapshot ticket was not released.");
        }
        failPollOnCall = 0;
        admissionError = -5;
        try { fetch(null); throw new Exception("Stopped owner was accepted."); }
        catch (InvalidOperationException) { }
        afterCommit();
        if (cached.GetValue(null) != null) throw new Exception("Failed post-commit refresh retained stale state.");
        Console.WriteLine("PASS snapshot initial/unchanged/changed reads, owner replacement and provider notification gate, expiry, explicit refresh and errors");
    }
}
