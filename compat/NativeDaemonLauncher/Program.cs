using System.Diagnostics;

// The pinned original UX watchdog launches this name. This project forwards
// ownership to the Rust daemon; it never constructs an upstream Driver/RootHub.
var name = OperatingSystem.IsLinux() ? "opentabletdriver-rust-linux" :
    OperatingSystem.IsMacOS() ? "opentabletdriver-rust-macos" : null;
if (name == null) { Console.Error.WriteLine("This launcher is for Linux/macOS."); return 1; }
var start = new ProcessStartInfo(Path.Combine(AppContext.BaseDirectory, name)) {
    WorkingDirectory = AppContext.BaseDirectory,
    UseShellExecute = false,
    RedirectStandardInput = true
};
start.ArgumentList.Add("daemon");
start.ArgumentList.Add("--upstream-pipe");
start.ArgumentList.Add("OpenTabletDriver.Daemon");
start.ArgumentList.Add("--owner-stdin");
try {
    using var daemon = Process.Start(start) ?? throw new IOException("Native daemon did not start.");
    // If the watchdog kills this launcher, closing the pipe signals the Rust
    // daemon to stop, join each reader and release held output on its owner.
    await daemon.WaitForExitAsync();
    return daemon.ExitCode;
} catch (Exception error) {
    Console.Error.WriteLine(error.Message);
    return 1;
}
