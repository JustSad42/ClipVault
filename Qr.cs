using System.Windows.Media;
using System.Windows.Media.Imaging;
using ZXing;
using ZXing.Common;

namespace ClipVault;

/// <summary>Finds and reads a QR code in an image (offline, using the ZXing library).</summary>
static class Qr
{
    /// <returns>The QR code's text, or "" when the image has none.</returns>
    public static Task<string> ReadAsync(string imagePath) => Task.Run(() =>
    {
        try
        {
            var img = new BitmapImage();
            img.BeginInit();
            img.CacheOption = BitmapCacheOption.OnLoad;
            img.CreateOptions = BitmapCreateOptions.IgnoreImageCache;
            img.UriSource = new Uri(imagePath);
            img.DecodePixelWidth = 0;
            img.EndInit();
            BitmapSource src = img;
            // Big screenshots: scale down for speed; QR codes stay readable.
            double scale = Math.Min(1, 1600.0 / Math.Max(src.PixelWidth, src.PixelHeight));
            if (scale < 1) src = new TransformedBitmap(src, new ScaleTransform(scale, scale));
            if (src.Format != PixelFormats.Bgra32) src = new FormatConvertedBitmap(src, PixelFormats.Bgra32, null, 0);

            int w = src.PixelWidth, h = src.PixelHeight;
            var pixels = new byte[w * h * 4];
            src.CopyPixels(pixels, w * 4, 0);

            var luminance = new RGBLuminanceSource(pixels, w, h, RGBLuminanceSource.BitmapFormat.BGRA32);
            var reader = new MultiFormatReader();
            var hints = new Dictionary<DecodeHintType, object>
            {
                [DecodeHintType.POSSIBLE_FORMATS] = new List<BarcodeFormat> { BarcodeFormat.QR_CODE },
                [DecodeHintType.TRY_HARDER] = true,
            };
            var result = reader.decode(new BinaryBitmap(new HybridBinarizer(luminance)), hints);
            return result?.Text?.Trim() ?? "";
        }
        catch { return ""; }
    });
}
