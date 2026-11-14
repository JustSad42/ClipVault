using WinImaging = global::Windows.Graphics.Imaging;
using WinOcr = global::Windows.Media.Ocr;
using WinStorage = global::Windows.Storage;

namespace ClipVault;

/// <summary>Reads text out of images using the OCR engine built into Windows (works offline).</summary>
static class Ocr
{
    static WinOcr.OcrEngine? _engine;
    static bool _initialized;

    public static async Task<string?> ReadAsync(string imagePath)
    {
        if (!_initialized)
        {
            _initialized = true;
            _engine = WinOcr.OcrEngine.TryCreateFromUserProfileLanguages();
        }
        if (_engine == null) return null;

        var file = await WinStorage.StorageFile.GetFileFromPathAsync(imagePath);
        using var stream = await file.OpenAsync(WinStorage.FileAccessMode.Read);
        var decoder = await WinImaging.BitmapDecoder.CreateAsync(stream);
        uint max = WinOcr.OcrEngine.MaxImageDimension;
        if (decoder.PixelWidth > max || decoder.PixelHeight > max || decoder.PixelWidth < 20 || decoder.PixelHeight < 20)
            return "";

        using var bitmap = await decoder.GetSoftwareBitmapAsync(
            WinImaging.BitmapPixelFormat.Bgra8, WinImaging.BitmapAlphaMode.Premultiplied);
        var result = await _engine.RecognizeAsync(bitmap);
        return result.Text;
    }
}
