using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;

namespace ClipVault;

/// <summary>Simple image edits for screenshots: crop, rotate and blur (pixelate) part of the image.</summary>
static class ImageEdit
{
    public static BitmapSource Load(string path)
    {
        var frame = BitmapFrame.Create(new Uri(path), BitmapCreateOptions.IgnoreImageCache, BitmapCacheOption.OnLoad);
        BitmapSource src = frame.Format == PixelFormats.Bgra32 ? frame : new FormatConvertedBitmap(frame, PixelFormats.Bgra32, null, 0);
        src.Freeze();
        return src;
    }

    public static BitmapSource Rotate(BitmapSource src)
    {
        var r = new TransformedBitmap(src, new RotateTransform(90));
        var w = new WriteableBitmap(r);
        w.Freeze();
        return w;
    }

    public static BitmapSource Crop(BitmapSource src, Int32Rect rect)
    {
        var c = new CroppedBitmap(src, rect);
        var w = new WriteableBitmap(c);
        w.Freeze();
        return w;
    }

    /// <summary>Pixelates a rectangle so text in it can't be read back.</summary>
    public static BitmapSource Pixelate(BitmapSource src, Int32Rect rect)
    {
        int w = src.PixelWidth, h = src.PixelHeight, stride = w * 4;
        var px = new byte[stride * h];
        src.CopyPixels(px, stride, 0);
        int block = Math.Max(8, Math.Min(w, h) / 45);

        for (int by = rect.Y; by < rect.Y + rect.Height; by += block)
        for (int bx = rect.X; bx < rect.X + rect.Width; bx += block)
        {
            int x1 = Math.Min(bx + block, rect.X + rect.Width), y1 = Math.Min(by + block, rect.Y + rect.Height);
            long b = 0, g = 0, r = 0, a = 0, n = 0;
            for (int y = by; y < y1; y++)
            for (int x = bx; x < x1; x++)
            {
                int i = y * stride + x * 4;
                b += px[i]; g += px[i + 1]; r += px[i + 2]; a += px[i + 3]; n++;
            }
            if (n == 0) continue;
            for (int y = by; y < y1; y++)
            for (int x = bx; x < x1; x++)
            {
                int i = y * stride + x * 4;
                px[i] = (byte)(b / n); px[i + 1] = (byte)(g / n); px[i + 2] = (byte)(r / n); px[i + 3] = (byte)(a / n);
            }
        }

        var result = BitmapSource.Create(w, h, src.DpiX, src.DpiY, PixelFormats.Bgra32, null, px, stride);
        result.Freeze();
        return result;
    }

    /// <summary>Clamps a rectangle to the image; returns null when too small to matter.</summary>
    public static Int32Rect? Clamp(BitmapSource src, double x0, double y0, double x1, double y1)
    {
        int l = (int)Math.Max(0, Math.Min(x0, x1)), t = (int)Math.Max(0, Math.Min(y0, y1));
        int r = (int)Math.Min(src.PixelWidth, Math.Max(x0, x1)), b = (int)Math.Min(src.PixelHeight, Math.Max(y0, y1));
        return r - l < 4 || b - t < 4 ? null : new Int32Rect(l, t, r - l, b - t);
    }
}
