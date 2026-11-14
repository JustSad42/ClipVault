using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Effects;

namespace ClipVault;

/// <summary>
/// A small confirmation bubble ("Copied", "Deleted · Undo") that fades in and out.
/// It never takes focus, so clicking its button doesn't close the popup.
/// </summary>
static class Toast
{
    static Window? _window;
    static TextBlock? _text;
    static Button? _button;
    static Action? _action;

    /// <param name="area">Screen area (WPF units) to center it in, near the bottom.</param>
    /// <param name="actionLabel">Optional button, e.g. "Undo". The toast then stays up longer.</param>
    public static void Show(string message, Rect area, string? actionLabel = null, Action? action = null)
    {
        _window ??= Create();
        _text!.Text = message;
        _action = action;
        _button!.Content = actionLabel;
        _button.Visibility = actionLabel != null && action != null ? Visibility.Visible : Visibility.Collapsed;

        _window.BeginAnimation(UIElement.OpacityProperty, null);
        _window.Opacity = 0;
        _window.Show();
        _window.UpdateLayout();
        _window.Left = area.Left + (area.Width - _window.ActualWidth) / 2;
        _window.Top = area.Bottom - _window.ActualHeight - 56;

        double hold = action != null ? 5000 : 1300;
        var fade = new DoubleAnimationUsingKeyFrames();
        fade.KeyFrames.Add(new EasingDoubleKeyFrame(1, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(140))));
        fade.KeyFrames.Add(new LinearDoubleKeyFrame(1, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(hold))));
        fade.KeyFrames.Add(new EasingDoubleKeyFrame(0, KeyTime.FromTimeSpan(TimeSpan.FromMilliseconds(hold + 300))));
        fade.Completed += (_, _) => { if (_window.Opacity == 0) { _window.Hide(); _action = null; } };
        _window.BeginAnimation(UIElement.OpacityProperty, fade);
    }

    public static void HideNow()
    {
        if (_window == null) return;
        _window.BeginAnimation(UIElement.OpacityProperty, null);
        _window.Opacity = 0;
        _window.Hide();
        _action = null;
    }

    static Window Create()
    {
        var res = Application.Current.Resources;
        _text = new TextBlock { VerticalAlignment = VerticalAlignment.Center, FontSize = 13 };
        _text.SetResourceReference(TextBlock.ForegroundProperty, "Fg");

        var check = new TextBlock
        {
            Text = "", FontFamily = (FontFamily)res["Icons"], FontSize = 12,
            Margin = new Thickness(0, 1, 8, 0), VerticalAlignment = VerticalAlignment.Center,
        };
        check.SetResourceReference(TextBlock.ForegroundProperty, "Accent");

        _button = new Button
        {
            Margin = new Thickness(14, -3, -6, -3), Padding = new Thickness(10, 3, 10, 4), MinWidth = 0,
            Focusable = false, Cursor = Cursors.Hand, FontWeight = FontWeights.SemiBold,
            Style = (Style)res["SecondaryButton"], Visibility = Visibility.Collapsed,
        };
        _button.SetResourceReference(Control.ForegroundProperty, "Accent");
        _button.Click += (_, _) => { var a = _action; HideNow(); a?.Invoke(); };

        var pill = new Border
        {
            CornerRadius = new CornerRadius(18), Padding = new Thickness(16, 8, 18, 9),
            BorderThickness = new Thickness(1), Margin = new Thickness(16),
            Child = new StackPanel { Orientation = Orientation.Horizontal, Children = { check, _text, _button } },
            Effect = new DropShadowEffect { BlurRadius = 16, ShadowDepth = 2, Opacity = 0.3 },
        };
        pill.SetResourceReference(Border.BackgroundProperty, "Bg");
        pill.SetResourceReference(Border.BorderBrushProperty, "Line");

        var w = new Window
        {
            WindowStyle = WindowStyle.None, AllowsTransparency = true, Background = Brushes.Transparent,
            ResizeMode = ResizeMode.NoResize, ShowInTaskbar = false, ShowActivated = false, Topmost = true,
            Focusable = false, SizeToContent = SizeToContent.WidthAndHeight,
            FontFamily = (FontFamily)res["UiFont"], Content = pill,
        };
        w.SourceInitialized += (_, _) => Native.AddExStyle(new WindowInteropHelper(w).Handle,
            Native.WS_EX_NOACTIVATE | Native.WS_EX_TOOLWINDOW);
        return w;
    }
}
