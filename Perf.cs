using System.Diagnostics;
using System.Text;

namespace ClipVault;

/// <summary>
/// Optional timing log for diagnosing slow popup opens. Off unless the CLIPVAULT_PERF environment
/// variable is set; then each open appends phase timings and CPU use to %TEMP%\clipvault-perf.log.
/// </summary>
sealed class Perf
{
    static readonly bool Enabled = Environment.GetEnvironmentVariable("CLIPVAULT_PERF") == "1";
    readonly Stopwatch _sw = Stopwatch.StartNew();
    readonly TimeSpan _cpu0 = Process.GetCurrentProcess().TotalProcessorTime;
    readonly StringBuilder _log = new();
    long _last;

    public static Perf? Start() => Enabled ? new Perf() : null;

    public void Mark(string phase)
    {
        long now = _sw.ElapsedMilliseconds;
        _log.Append($"{phase} +{now - _last}ms; ");
        _last = now;
    }

    public void Write()
    {
        var cpu = Process.GetCurrentProcess().TotalProcessorTime - _cpu0;
        var line = $"{DateTime.Now:HH:mm:ss} total {_sw.ElapsedMilliseconds}ms cpu {cpu.TotalMilliseconds:0}ms gc-heap {GC.GetTotalMemory(false) / 1048576}MB | {_log}";
        try { File.AppendAllText(Path.Combine(Path.GetTempPath(), "clipvault-perf.log"), line + Environment.NewLine); } catch { }
    }
}
