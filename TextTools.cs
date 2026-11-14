using System.Globalization;
using System.Text.RegularExpressions;

namespace ClipVault;

/// <summary>The "Paste as…" text transformations.</summary>
static class TextTools
{
    public static string Apply(string kind, string text) => kind switch
    {
        "upper" => text.ToUpper(CultureInfo.CurrentCulture),
        "lower" => text.ToLower(CultureInfo.CurrentCulture),
        "title" => CultureInfo.CurrentCulture.TextInfo.ToTitleCase(text.ToLower(CultureInfo.CurrentCulture)),
        "trim" => TrimSpaces(text),
        "single" => Regex.Replace(text.Trim(), @"\s*\r?\n\s*", " "),
        _ => text,
    };

    /// <summary>Trims each line and collapses repeated spaces/tabs, keeping the line breaks.</summary>
    static string TrimSpaces(string text)
    {
        var lines = text.Replace("\r\n", "\n").Split('\n')
            .Select(l => Regex.Replace(l.Trim(), @"[ \t]{2,}", " "));
        return string.Join(Environment.NewLine, lines).Trim();
    }
}
