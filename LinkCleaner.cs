namespace ClipVault;

/// <summary>Removes tracking parameters (utm_source, fbclid, …) from copied links.</summary>
static class LinkCleaner
{
    static readonly HashSet<string> Names = new(StringComparer.OrdinalIgnoreCase)
    {
        "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "ttclid",
        "igshid", "igsh", "mc_cid", "mc_eid", "mkt_tok", "_hsenc", "_hsmi", "hsctatracking", "li_fat_id",
        "vero_id", "vero_conv", "oly_anon_id", "oly_enc_id", "wickedid", "rb_clickid", "s_cid", "ref_src", "ref_url",
        "_ga", "_gl", "trk", "trkcampaign", "sc_campaign", "sc_channel", "sc_content", "sc_medium", "sc_outcome",
        "sc_geo", "sc_country", "cmpid", "srsltid", "si_source",
    };

    static readonly string[] Prefixes = ["utm_", "pk_", "hsa_", "pd_rd_", "pf_rd_", "mtm_"];

    /// <summary>"si" is a share-tracking id on these sites, but a real parameter elsewhere.</summary>
    static readonly string[] SiHosts = ["youtube.com", "youtu.be", "spotify.com", "instagram.com", "threads.net"];

    /// <summary>Returns the link without tracking parameters, or null when there was nothing to remove.</summary>
    public static string? Clean(string text)
    {
        var t = text.Trim();
        if (t.Length > 4000 || t.Any(char.IsWhiteSpace)) return null;
        if (!Uri.TryCreate(t, UriKind.Absolute, out var uri) || (uri.Scheme != "http" && uri.Scheme != "https")) return null;

        int q = t.IndexOf('?');
        if (q < 0) return null;
        int hash = t.IndexOf('#', q);
        var query = hash < 0 ? t[(q + 1)..] : t[(q + 1)..hash];
        var fragment = hash < 0 ? "" : t[hash..];
        bool siTracking = SiHosts.Any(h => uri.Host.Equals(h, StringComparison.OrdinalIgnoreCase) || uri.Host.EndsWith("." + h, StringComparison.OrdinalIgnoreCase));

        var kept = new List<string>();
        bool removed = false;
        foreach (var part in query.Split('&'))
        {
            if (part.Length == 0) continue;
            var name = Uri.UnescapeDataString(part.Split('=')[0]);
            bool track = Names.Contains(name)
                         || Prefixes.Any(p => name.StartsWith(p, StringComparison.OrdinalIgnoreCase))
                         || (siTracking && name.Equals("si", StringComparison.OrdinalIgnoreCase));
            if (track) removed = true; else kept.Add(part);
        }
        if (!removed) return null;
        return t[..q] + (kept.Count > 0 ? "?" + string.Join("&", kept) : "") + fragment;
    }
}
