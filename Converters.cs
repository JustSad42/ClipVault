using System.Globalization;
using System.Windows;
using System.Windows.Data;

namespace ClipVault;

/// <summary>Visible when the value is set; Collapsed when null. ConverterParameter="invert" flips it.</summary>
public sealed class NullToVisibility : IValueConverter
{
    public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture)
    {
        bool show = value != null;
        if (parameter as string == "invert") show = !show;
        return show ? Visibility.Visible : Visibility.Collapsed;
    }

    public object ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}

/// <summary>Visible when the item's Card matches ConverterParameter (e.g. "Link"); several can be given as "Link,Files".</summary>
public sealed class CardIs : IValueConverter
{
    public object Convert(object? value, Type targetType, object? parameter, CultureInfo culture) =>
        value != null && (parameter as string ?? "").Split(',').Contains(value.ToString()) ? Visibility.Visible : Visibility.Collapsed;

    public object ConvertBack(object? value, Type targetType, object? parameter, CultureInfo culture) =>
        throw new NotSupportedException();
}
