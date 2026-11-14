using System.Media;

namespace ClipVault;

/// <summary>Soft, short sounds for copying and pasting. Generated in code, so nothing extra ships with the app.</summary>
static class Sounds
{
    static SoundPlayer? _copy, _paste;

    public static void Copy()
    {
        if (!((App)System.Windows.Application.Current).Settings.Sounds) return;
        (_copy ??= Make([(1320, 0.045)], 0.16)).Play();
    }

    public static void Paste()
    {
        if (!((App)System.Windows.Application.Current).Settings.Sounds) return;
        (_paste ??= Make([(880, 0.04), (1175, 0.06)], 0.16)).Play();
    }

    /// <summary>Builds a small WAV of gentle sine tones with soft starts and endings.</summary>
    static SoundPlayer Make((double Hz, double Seconds)[] tones, double volume)
    {
        const int rate = 44100;
        var samples = new List<short>();
        foreach (var (hz, secs) in tones)
        {
            int n = (int)(rate * secs);
            for (int i = 0; i < n; i++)
            {
                double t = (double)i / rate;
                double env = Math.Min(1, i / (rate * 0.004)) * Math.Pow(1 - (double)i / n, 2); // quick fade-in, smooth fade-out
                samples.Add((short)(Math.Sin(2 * Math.PI * hz * t) * env * volume * short.MaxValue));
            }
        }

        var ms = new MemoryStream();
        var w = new BinaryWriter(ms);
        int dataBytes = samples.Count * 2;
        w.Write("RIFF"u8); w.Write(36 + dataBytes); w.Write("WAVE"u8);
        w.Write("fmt "u8); w.Write(16); w.Write((short)1); w.Write((short)1); w.Write(rate); w.Write(rate * 2); w.Write((short)2); w.Write((short)16);
        w.Write("data"u8); w.Write(dataBytes);
        foreach (var s in samples) w.Write(s);
        ms.Position = 0;
        var player = new SoundPlayer(ms);
        player.Load();
        return player;
    }
}
