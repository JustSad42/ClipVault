using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Data;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Microsoft.Win32;

namespace ClipVault;

enum PasteMode { Normal, Plain, CopyOnly }
enum PopupMode { List, Preview, Edit, ImageEdit }
enum FilterMode { All, Text, Images, Files, Pinned }

public partial class MainWindow : Window
{
    readonly App _app;
    readonly ListCollectionView _view;
    readonly RadioButton[] _filterButtons;
    readonly List<ClipItem> _badged = new();
    IntPtr _target;          // window that was active before the popup opened; pastes go there
    bool _keepOpen;          // true while a dialog or drag of ours is running, so losing focus doesn't hide the popup
    bool _gridMode;          // Images tab shows a thumbnail grid
    string _layoutKey = "";  // which row look is applied: list, compact or grid
    bool _moved;             // popup was dragged since it opened
    List<ClipItem> _lastDeleted = new();
    // Image editor state
    ClipItem? _imgItem;
    BitmapSource? _imgCurrent;
    readonly List<BitmapSource> _imgHistory = new();
    Point? _imgDragStart;
    DateTime _openedAt;
    PopupMode _mode = PopupMode.List;
    FilterMode _filter = FilterMode.All;
    string[] _words = [];
    List<Func<ClipItem, bool>> _filters = new();
    readonly DispatcherTimer _trimTimer = new() { Interval = TimeSpan.FromSeconds(3) };
    ClipItem? _editing;
    Point _dragStart;
    ClipItem? _dragItem;

    public MainWindow(App app)
    {
        _app = app;
        InitializeComponent();
        Backdrop.Attach(this);
        Width = Math.Clamp(app.Settings.PopupWidth, MinWidth, MaxWidth);
        Height = Math.Clamp(app.Settings.PopupHeight, MinHeight, MaxHeight);
        _filterButtons = [FilterAll, FilterText, FilterImages, FilterFiles, FilterPinned];

        _view = (ListCollectionView)CollectionViewSource.GetDefaultView(app.Store.Items);
        _view.IsLiveSorting = true;
        _view.LiveSortingProperties.Add(nameof(ClipItem.Pinned));
        _view.LiveSortingProperties.Add(nameof(ClipItem.CopiedAt));
        _view.IsLiveGrouping = true;
        _view.LiveGroupingProperties.Add(nameof(ClipItem.Group));
        _view.Filter = Matches;
        ApplyLayout();
        List.ItemsSource = _view;

        // A few seconds after the popup closes: install a downloaded update while nothing is on screen,
        // and delete the files of removed items. Memory is deliberately left alone: dropping the
        // thumbnails and emptying the working set made the next open load everything back.
        _trimTimer.Tick += (_, _) =>
        {
            _trimTimer.Stop();
            if (IsVisible) return;
            if (Updater.Ready != null && Updater.ApplyAndRestart()) return;
            _app.Store.PurgeDeleted(TimeSpan.FromSeconds(10));
        };
        IsVisibleChanged += (_, _) =>
        {
            if (IsVisible) { _trimTimer.Stop(); return; }
            SaveWindowPlacement();
            _trimTimer.Start();
        };

        List.SelectionChanged += (_, _) => { if (_mode == PopupMode.Preview) PreviewPane.DataContext = Selected; };

        // New copies arriving while the popup is open: renumber after the list has updated.
        app.Store.Items.CollectionChanged += (_, _) =>
            Dispatcher.BeginInvoke(() => { UpdateBadges(); UpdateEmptyState(); });
    }

    ClipItem? Selected => List.SelectedItem as ClipItem;
    Rect Area => new(Left, Top, ActualWidth > 0 ? ActualWidth : Width, ActualHeight > 0 ? ActualHeight : Height);

    // ---------- showing / hiding ----------

    /// <summary>Hotkey handler: open near the text cursor, or close if already open.</summary>
    public void Toggle(IntPtr foreground)
    {
        if (IsVisible) { Dismiss(); return; }
        _target = foreground;
        Open(center: false);
    }

    /// <summary>Open in the middle of the screen (used on manual launch). Selecting copies but doesn't paste.</summary>
    public void ShowCentered()
    {
        if (IsVisible) { Activate(); return; }
        _target = IntPtr.Zero;
        Open(center: true);
    }

    void Open(bool center)
    {
        var perf = Perf.Start();
        bool themeChanged = _app.ApplyTheme();
        perf?.Mark("theme");
        ResetModes();
        SearchBox.Text = "";
        FilterAll.IsChecked = true;
        perf?.Mark("reset");
        // Only "2m ago" texts, and headings for items that crossed midnight, need updating.
        foreach (var item in _app.Store.Items) item.RefreshMeta(themeChanged);
        perf?.Mark("meta");
        RefreshList(null, force: false);
        perf?.Mark("list");

        _moved = false;
        bool above = PlaceWindow(center);
        _openedAt = DateTime.Now;
        Show();
        perf?.Mark("show");
        Native.ForceForeground(new WindowInteropHelper(this).Handle);
        Activate();
        SearchBox.Focus();
        Keyboard.Focus(SearchBox);
        AnimateIn(fromAbove: above);
        perf?.Mark("activate");
        if (perf != null)
        {
            int frames = 0;
            EventHandler onFrame = (_, _) => frames++;
            CompositionTarget.Rendering += onFrame;
            Dispatcher.BeginInvoke(() => perf.Mark("layout+first render"), DispatcherPriority.Loaded);
            Dispatcher.BeginInvoke(async () =>
            {
                await Task.Delay(600);
                CompositionTarget.Rendering -= onFrame;
                perf.Mark($"settled, {frames} frames, render tier {RenderCapability.Tier >> 16}");
                perf.Write();
            }, DispatcherPriority.ContextIdle);
        }
    }

    /// <summary>A very short fade and slide toward the anchor: visible, but nothing to wait for.</summary>
    void AnimateIn(bool fromAbove)
    {
        var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
        Body.BeginAnimation(OpacityProperty, new DoubleAnimation(0.4, 1, TimeSpan.FromMilliseconds(60)) { EasingFunction = ease });
        BodyShift.BeginAnimation(TranslateTransform.YProperty,
            new DoubleAnimation(fromAbove ? 4 : -4, 0, TimeSpan.FromMilliseconds(90)) { EasingFunction = ease });
    }

    /// <summary>Closing is instant: a fade only delays getting back to what you were doing.</summary>
    void CloseNow()
    {
        if (IsVisible) Hide();
    }

    /// <returns>True when the popup opens above the anchor (so it slides up).</returns>
    bool PlaceWindow(bool center)
    {
        var hwnd = new WindowInteropHelper(this).EnsureHandle();

        // Anchor: the text cursor in the app you were typing in, else the mouse.
        Native.GetCursorPos(out var mouse);
        Native.RECT caret = default;
        bool hasCaret = !center && _app.Settings.OpenAt == OpenPosition.TextCursor
                        && Native.TryGetCaretRect(_target, out caret);
        var anchor = hasCaret ? new Native.POINT { X = caret.Left, Y = caret.Bottom } : mouse;

        var monitor = Native.MonitorFromPoint(anchor, 2 /* MONITOR_DEFAULTTONEAREST */);
        var info = new Native.MONITORINFO { cbSize = Marshal.SizeOf<Native.MONITORINFO>() };
        Native.GetMonitorInfo(monitor, ref info);

        // Convert screen pixels to WPF units (handles display scaling).
        var toDip = HwndSource.FromHwnd(hwnd).CompositionTarget.TransformFromDevice;
        var workTL = toDip.Transform(new Point(info.rcWork.Left, info.rcWork.Top));
        var workBR = toDip.Transform(new Point(info.rcWork.Right, info.rcWork.Bottom));

        double x, y;
        bool above = false;
        if (!center && _app.Settings.OpenAt == OpenPosition.LastPosition && _app.Settings.PopupLeft is { } savedX && _app.Settings.PopupTop is { } savedY)
        {
            // Where it was last left, kept inside the visible screen area.
            Left = Math.Clamp(savedX, SystemParameters.VirtualScreenLeft, SystemParameters.VirtualScreenLeft + SystemParameters.VirtualScreenWidth - Width);
            Top = Math.Clamp(savedY, SystemParameters.VirtualScreenTop, SystemParameters.VirtualScreenTop + SystemParameters.VirtualScreenHeight - Height);
            return false;
        }
        if (center)
        {
            x = (workTL.X + workBR.X - Width) / 2;
            y = (workTL.Y + workBR.Y - Height) / 2;
        }
        else if (hasCaret)
        {
            var top = toDip.Transform(new Point(caret.Left, caret.Top));
            var bottom = toDip.Transform(new Point(caret.Left, caret.Bottom));
            x = top.X;
            y = bottom.Y + 6;
            if (y + Height > workBR.Y) { y = top.Y - 6 - Height; above = true; } // no room below: open above the line
        }
        else
        {
            var p = toDip.Transform(new Point(mouse.X, mouse.Y));
            x = p.X;
            y = p.Y;
            if (y + Height > workBR.Y) above = true;
        }
        Left = Math.Max(workTL.X, Math.Min(x, workBR.X - Width));
        Top = Math.Max(workTL.Y, Math.Min(y, workBR.Y - Height));
        return above;
    }

    void Dismiss()
    {
        var target = _target;
        CloseNow();
        if (target != IntPtr.Zero) Native.SetForegroundWindow(target);
    }

    async void OnDeactivated(object? sender, EventArgs e)
    {
        if (_keepOpen) return;
        // Windows sometimes bounces focus back to the previous app while the popup appears: in that
        // first moment, look again shortly before closing (a real click elsewhere still closes it).
        if ((DateTime.Now - _openedAt).TotalMilliseconds < 150)
        {
            await Task.Delay(150);
            if (_keepOpen || IsActive) return;
        }
        CloseNow();
    }

    // ---------- filtering / layout ----------

    bool Matches(object o)
    {
        var item = (ClipItem)o;
        bool kindOk = _filter switch
        {
            FilterMode.Text => item.IsText,
            FilterMode.Images => item.IsImage,
            FilterMode.Files => item.IsFiles,
            FilterMode.Pinned => item.Pinned,
            _ => true,
        };
        if (!kindOk) return false;
        foreach (var f in _filters) if (!f(item)) return false;
        if (_words.Length == 0) return true;
        item.SearchRank = Search.Score(item, _words);
        return item.SearchRank > 0;
    }

    /// <summary>
    /// Sets sorting, date headings and list/grid look for the current tab and search, then re-filters.
    /// While searching: best matches first, no headings. Images tab: thumbnail grid.
    /// </summary>
    void ApplyLayout()
    {
        bool searching = _words.Length > 0;
        bool grid = _filter == FilterMode.Images;
        // Rebuilding sorting and headings makes WPF recreate every row, so only do it when the
        // search text or tab actually changed (not on every open).
        var query = $"{_filter}|{SearchBox.Text}";
        if (query == _lastQuery) { ApplyTemplates(grid); return; }
        _lastQuery = query;
        var shape = $"{searching}|{grid}";
        if (shape == _lastShape) { _view.Refresh(); ApplyTemplates(grid); return; }
        _lastShape = shape;
        using (_view.DeferRefresh())
        {
            _view.SortDescriptions.Clear();
            _view.SortDescriptions.Add(searching
                ? new SortDescription(nameof(ClipItem.SearchRank), ListSortDirection.Descending)
                : new SortDescription(nameof(ClipItem.Pinned), ListSortDirection.Descending));
            _view.SortDescriptions.Add(new SortDescription(nameof(ClipItem.CopiedAt), ListSortDirection.Descending));
            _view.GroupDescriptions.Clear();
            if (!searching && !grid) _view.GroupDescriptions.Add(new PropertyGroupDescription(nameof(ClipItem.Group)));
        }
        ApplyTemplates(grid);
    }

    string? _lastQuery, _lastShape;

    void ApplyTemplates(bool grid)
    {
        _gridMode = grid;
        var key = grid ? "grid" : _app.Settings.Compact ? "compact" : "list";
        if (key == _layoutKey) return;
        _layoutKey = key;
        List.ItemContainerStyle = (Style)Resources[key switch { "grid" => "TileStyle", "compact" => "CompactRowStyle", _ => "RowStyle" }];
        List.ItemTemplate = (DataTemplate)Resources[key switch { "grid" => "TileTemplate", "compact" => "CompactRowTemplate", _ => "RowTemplate" }];
        List.ItemsPanel = (ItemsPanelTemplate)Resources[grid ? "GridPanel" : "ListPanel"];
    }

    void OnSearchChanged(object sender, TextChangedEventArgs e) => RefreshList(null, force: false);
    void OnClearSearch(object sender, RoutedEventArgs e) { SearchBox.Text = ""; SearchBox.Focus(); }

    void OnFilterChecked(object sender, RoutedEventArgs e)
    {
        _filter = Enum.Parse<FilterMode>((string)((RadioButton)sender).Tag);
        if (_view != null) RefreshList(null, force: false);
    }

    void CycleFilter(int step)
    {
        int i = Array.FindIndex(_filterButtons, b => b.IsChecked == true);
        _filterButtons[(i + step + _filterButtons.Length) % _filterButtons.Length].IsChecked = true;
    }

    /// <summary>Re-applies search, tab and sort, renumbers the 1–9 badges and keeps a sensible selection.</summary>
    /// <param name="force">Re-filter even if the search and tab are unchanged (after edits, pins, deletes).</param>
    void RefreshList(ClipItem? keep, bool force = true)
    {
        if (force) _lastQuery = null;
        (_words, _filters) = Search.Parse(SearchBox.Text);
        Highlighter.SetQuery(string.Join(" ", _words));
        ApplyLayout(); // also refreshes the view
        UpdateEmptyState();
        UpdateBadges();
        if (keep != null && List.Items.Contains(keep))
        {
            List.SelectedItem = keep;
            List.ScrollIntoView(keep);
        }
        else SelectIndex(0);
    }

    void UpdateBadges()
    {
        foreach (var item in _badged) item.Badge = null;
        _badged.Clear();
        for (int i = 0; i < Math.Min(9, List.Items.Count); i++)
        {
            var item = (ClipItem)List.Items[i]!;
            item.Badge = (i + 1).ToString();
            _badged.Add(item);
        }
    }

    void UpdateEmptyState()
    {
        bool empty = List.Items.Count == 0 && _mode == PopupMode.List;
        EmptyState.Visibility = empty ? Visibility.Visible : Visibility.Collapsed;
        if (!empty) return;

        var (glyph, title, hint) =
            _app.Store.Items.Count == 0 ? ("", "Nothing here yet", "Copy some text, an image or a file and it will show up here.")
            : _words.Length > 0 || _filters.Count > 0 ? ("", "No matches", _filter == FilterMode.All
                ? "Try fewer or different words. Text inside images is searched too."
                : "Press Tab to look in the other tabs.")
            : _filter switch
            {
                FilterMode.Pinned => ("", "No pinned items", "Select an item and press Ctrl+P to keep it here for good."),
                FilterMode.Images => ("", "No images yet", "Screenshots and copied pictures show up here."),
                FilterMode.Files => ("", "No files yet", "Files you copy in File Explorer show up here."),
                _ => ("", "No text yet", "Text you copy shows up here."),
            };
        EmptyGlyph.Text = glyph;
        EmptyTitle.Text = title;
        EmptyHint.Text = hint;
    }

    void SelectIndex(int index)
    {
        if (List.Items.Count == 0) return;
        List.SelectedIndex = Math.Clamp(index, 0, List.Items.Count - 1);
        List.ScrollIntoView(List.SelectedItem);
    }

    // ---------- preview / editor ----------

    void ResetModes()
    {
        PreviewPane.Visibility = Visibility.Collapsed;
        PreviewPane.DataContext = null;
        EditorPane.Visibility = Visibility.Collapsed;
        ImageEditorPane.Visibility = Visibility.Collapsed;
        List.Visibility = Visibility.Visible;
        _imgItem = null;
        _imgCurrent = null;
        _imgHistory.Clear();
        EditImage.Source = null;
        _mode = PopupMode.List;
        _editing = null;
    }

    void TogglePreview()
    {
        if (_mode == PopupMode.Preview) { ClosePreview(); return; }
        if (Selected == null) return;
        _mode = PopupMode.Preview;
        List.Visibility = Visibility.Hidden; // the glass shows through, so hide the list underneath
        PreviewPane.DataContext = Selected;
        PreviewPane.Visibility = Visibility.Visible;
        PreviewScroll.ScrollToTop();
    }

    void ClosePreview()
    {
        if (_mode != PopupMode.Preview) return;
        ResetModes();
        if (Selected != null) List.ScrollIntoView(Selected);
    }

    void OpenEditor(ClipItem item)
    {
        if (item.IsImage) { OpenImageEditor(item); return; }
        if (!item.IsText) return;
        ResetModes();
        _editing = item;
        _mode = PopupMode.Edit;
        EditorBox.Text = item.Text ?? "";
        List.Visibility = Visibility.Hidden;
        EmptyState.Visibility = Visibility.Collapsed;
        EditorPane.Visibility = Visibility.Visible;
        EditorBox.Focus();
        EditorBox.CaretIndex = EditorBox.Text.Length;
    }

    void CloseEditor()
    {
        ResetModes();
        SearchBox.Focus();
    }

    void SaveEdit(bool paste)
    {
        var item = _editing;
        if (item == null) return;
        if (EditorBox.Text != item.Text) _app.Store.UpdateText(item, EditorBox.Text);
        CloseEditor();
        if (paste) Paste(item, PasteMode.Normal);
        else
        {
            RefreshList(item);
            Toast.Show("Saved", Area);
        }
    }

    void OnEditCancel(object sender, RoutedEventArgs e) => CloseEditor();
    void OnEditSave(object sender, RoutedEventArgs e) => SaveEdit(false);
    void OnEditSavePaste(object sender, RoutedEventArgs e) => SaveEdit(true);

    // ---------- actions ----------

    /// <param name="replacementText">If set, pastes this text instead of the item (used by "Paste as…").</param>
    async void Paste(ClipItem item, PasteMode mode, string? replacementText = null)
    {
        // Apps on the "always plain text" list get plain text.
        if (mode == PasteMode.Normal && TargetWantsPlain()) mode = PasteMode.Plain;
        if (replacementText != null) _app.PutText(replacementText);
        else _app.PutOnClipboard(item, plain: mode == PasteMode.Plain);
        if (mode != PasteMode.CopyOnly) Sounds.Paste();

        var target = _target;
        var area = Area;
        CloseNow();
        if (mode == PasteMode.CopyOnly) Toast.Show("Copied to clipboard", area);
        if (target == IntPtr.Zero) return;
        Native.SetForegroundWindow(target);
        if (mode == PasteMode.CopyOnly) return;
        // Paste as soon as the target really has the focus (usually at once), not after a fixed wait.
        for (int waited = 0; waited < 200 && Native.GetForegroundWindow() != target; waited += 5) await Task.Delay(5);
        await Task.Delay(10); // let it route the focus to its text box
        Native.SendCtrlV();
    }

    void TogglePin(ClipItem item)
    {
        item.Pinned = !item.Pinned;
        _app.Store.ScheduleSave();
        RefreshList(item);
        Toast.Show(item.Pinned ? "Pinned" : "Unpinned", Area);
    }

    void Delete(ClipItem item)
    {
        int index = List.SelectedIndex;
        _app.Store.Remove(item);
        _lastDeleted = [item];
        Toast.Show("Deleted", Area, "Undo", UndoDelete);
        _view.Refresh();
        UpdateEmptyState();
        UpdateBadges();
        SelectIndex(index);
        if (_mode == PopupMode.Preview && Selected == null) ClosePreview();
    }

    // ---------- input ----------

    static int DigitOf(Key key) => key switch
    {
        >= Key.D1 and <= Key.D9 => key - Key.D0,
        >= Key.NumPad1 and <= Key.NumPad9 => key - Key.NumPad0,
        _ => 0,
    };

    void OnPreviewKeyDown(object sender, KeyEventArgs e)
    {
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        bool ctrl = Keyboard.Modifiers.HasFlag(ModifierKeys.Control);
        bool shift = Keyboard.Modifiers.HasFlag(ModifierKeys.Shift);
        bool alt = Keyboard.Modifiers.HasFlag(ModifierKeys.Alt);

        if (_mode == PopupMode.Edit)
        {
            if (key == Key.Escape) { CloseEditor(); e.Handled = true; }
            else if (ctrl && key == Key.S) { SaveEdit(false); e.Handled = true; }
            else if (ctrl && key == Key.Enter) { SaveEdit(true); e.Handled = true; }
            return;
        }

        if (_mode == PopupMode.ImageEdit)
        {
            if (key == Key.Escape) { ResetModes(); SearchBox.Focus(); }
            else if (ctrl && key == Key.Z) ImgUndo();
            else if (key == Key.R && !ctrl) ImgRotate();
            else if (key == Key.C && !ctrl) ToolCrop.IsChecked = true;
            else if (key == Key.B && !ctrl) ToolBlur.IsChecked = true;
            else if (ctrl && key == Key.S) SaveImageEdit(false);
            else if (ctrl && key == Key.Enter) SaveImageEdit(true);
            else return;
            e.Handled = true;
            return;
        }

        if (ctrl && key == Key.Z && _lastDeleted.Count > 0 && !(SearchBox.IsKeyboardFocused && SearchBox.CanUndo))
        {
            UndoDelete();
            e.Handled = true;
            return;
        }

        bool searchEmpty = SearchBox.Text.Length == 0;
        int row = _gridMode ? 3 : 1; // grid: up/down jump a whole row

        // Quick paste: 1–9 when the search box is empty, Alt+1–9 any time.
        int digit = DigitOf(key);
        if (digit > 0 && (alt || (searchEmpty && !ctrl && !shift)))
        {
            if (digit - 1 < List.Items.Count) Paste((ClipItem)List.Items[digit - 1]!, PasteMode.Normal);
            e.Handled = true;
            return;
        }

        switch (key)
        {
            case Key.Escape:
                if (_mode == PopupMode.Preview) ClosePreview();
                else if (!searchEmpty) SearchBox.Text = "";
                else Dismiss();
                e.Handled = true;
                break;
            case Key.Space when searchEmpty || shift:
                TogglePreview();
                e.Handled = true;
                break;
            case Key.Tab:
                CycleFilter(shift ? -1 : 1);
                e.Handled = true;
                break;
            case Key.F2:
                if (Selected is { } toEdit) OpenEditor(toEdit);
                e.Handled = true;
                break;
            case Key.Down: SelectIndex(List.SelectedIndex + row); e.Handled = true; break;
            case Key.Up: SelectIndex(List.SelectedIndex - row); e.Handled = true; break;
            case Key.Right when _gridMode && searchEmpty: SelectIndex(List.SelectedIndex + 1); e.Handled = true; break;
            case Key.Left when _gridMode && searchEmpty: SelectIndex(List.SelectedIndex - 1); e.Handled = true; break;
            case Key.PageDown: SelectIndex(List.SelectedIndex + 6 * row); e.Handled = true; break;
            case Key.PageUp: SelectIndex(List.SelectedIndex - 6 * row); e.Handled = true; break;
            case Key.Enter:
                if (Selected is { } toPaste)
                    Paste(toPaste, ctrl ? PasteMode.CopyOnly : shift ? PasteMode.Plain : PasteMode.Normal);
                e.Handled = true;
                break;
            case Key.O when ctrl:
                if (Selected is { HasQuickAction: true } toOpen && toOpen.Card != CardKind.Image) DoQuickAction(toOpen);
                e.Handled = true;
                break;
            case Key.S when ctrl:
                if (Selected is { CanSaveToFile: true } toSave) SaveToFile(toSave);
                e.Handled = true;
                break;
            case Key.P when ctrl:
                if (Selected is { } toPin) TogglePin(toPin);
                e.Handled = true;
                break;
            case Key.Delete when shift || searchEmpty:
                if (Selected is { } toDelete) Delete(toDelete);
                e.Handled = true;
                break;
        }
    }

    void OnItemClick(object sender, MouseButtonEventArgs e)
    {
        // Only react to clicks inside the row itself (not e.g. its right-click menu).
        if (sender is ListBoxItem { DataContext: ClipItem item } row &&
            e.OriginalSource is Visual v && v.IsDescendantOf(row))
        {
            Paste(item, PasteMode.Normal);
        }
    }

    void OnPinClick(object sender, RoutedEventArgs e)
    {
        if (((FrameworkElement)sender).DataContext is ClipItem item) TogglePin(item);
        e.Handled = true;
    }

    // ---------- drag and drop ----------

    void OnItemMouseDown(object sender, MouseButtonEventArgs e)
    {
        // Don't start drags from the pin button.
        bool onButton = e.OriginalSource is Visual v && FindParent<ButtonBase>(v) != null;
        _dragItem = onButton ? null : (sender as ListBoxItem)?.DataContext as ClipItem;
        _dragStart = e.GetPosition(this);
    }

    void OnItemMouseMove(object sender, MouseEventArgs e)
    {
        if (_dragItem == null || e.LeftButton != MouseButtonState.Pressed) return;
        var moved = e.GetPosition(this) - _dragStart;
        if (Math.Abs(moved.X) < SystemParameters.MinimumHorizontalDragDistance &&
            Math.Abs(moved.Y) < SystemParameters.MinimumVerticalDragDistance) return;

        var item = _dragItem;
        _dragItem = null;
        DataObject data;
        try { data = App.BuildData(item, plain: false, forDrag: true); }
        catch { return; }

        _keepOpen = true;
        DragDropEffects result;
        try { result = DragDrop.DoDragDrop((DependencyObject)sender, data, DragDropEffects.Copy); }
        finally { _keepOpen = false; }

        if (result != DragDropEffects.None)
        {
            Hide();
        }
    }

    static T? FindParent<T>(DependencyObject? d) where T : DependencyObject
    {
        while (d != null)
        {
            if (d is T t) return t;
            d = d is Visual ? VisualTreeHelper.GetParent(d) : LogicalTreeHelper.GetParent(d);
        }
        return null;
    }

    // ---------- card actions ----------

    void OnActionClick(object sender, RoutedEventArgs e)
    {
        if (((FrameworkElement)sender).DataContext is ClipItem item) DoQuickAction(item);
        e.Handled = true;
    }

    /// <summary>Open link, show file in Explorer, or save image, depending on the card.</summary>
    void DoQuickAction(ClipItem item)
    {
        switch (item.Card)
        {
            case CardKind.Link:
                try { Process.Start(new ProcessStartInfo((item.Text ?? "").Trim()) { UseShellExecute = true }); }
                catch { Toast.Show("Couldn't open the link", Area); return; }
                Hide();
                break;
            case CardKind.Files:
                var path = item.FilePaths.FirstOrDefault(p => File.Exists(p) || Directory.Exists(p));
                if (path == null) { Toast.Show("That file no longer exists", Area); return; }
                Process.Start("explorer.exe", $"/select,\"{path}\"");
                Hide();
                break;
            case CardKind.Image:
                SaveToFile(item);
                break;
        }
    }

    /// <summary>Saves a text item as a .txt file or an image as .png / .jpg.</summary>
    void SaveToFile(ClipItem item)
    {
        var dialog = new SaveFileDialog { Title = "Save to file", AddExtension = true, OverwritePrompt = true };
        if (item.IsImage)
        {
            dialog.FileName = $"Image {item.CopiedAt:yyyy-MM-dd HHmmss}.png";
            dialog.Filter = "PNG image|*.png|JPEG image|*.jpg";
        }
        else
        {
            dialog.FileName = SuggestFileName(item.Text ?? "") + ".txt";
            dialog.Filter = "Text file|*.txt|All files|*.*";
        }

        _keepOpen = true;
        bool ok;
        try { ok = dialog.ShowDialog(this) == true; }
        finally { _keepOpen = false; }
        Activate();
        SearchBox.Focus();
        if (!ok) return;

        try
        {
            if (!item.IsImage) File.WriteAllText(dialog.FileName, item.Text ?? "");
            else if (dialog.FilterIndex == 1 || dialog.FileName.EndsWith(".png", StringComparison.OrdinalIgnoreCase))
                File.Copy(item.ImagePath!, dialog.FileName, true);
            else
            {
                var frame = BitmapFrame.Create(new Uri(item.ImagePath!), BitmapCreateOptions.IgnoreImageCache, BitmapCacheOption.OnLoad);
                var encoder = new JpegBitmapEncoder { QualityLevel = 92 };
                encoder.Frames.Add(BitmapFrame.Create(new FormatConvertedBitmap(frame, PixelFormats.Bgr24, null, 0)));
                using var stream = File.Create(dialog.FileName);
                encoder.Save(stream);
            }
            Toast.Show($"Saved {Path.GetFileName(dialog.FileName)}", Area);
        }
        catch (Exception ex)
        {
            Toast.Show("Couldn't save: " + ex.Message, Area);
        }
    }

    /// <summary>First few words of the text, made safe for a file name.</summary>
    static string SuggestFileName(string text)
    {
        var line = text.Trim().Split('\n')[0].Trim();
        var bad = Path.GetInvalidFileNameChars();
        var name = new string(line.Select(c => bad.Contains(c) ? ' ' : c).ToArray()).Trim();
        name = System.Text.RegularExpressions.Regex.Replace(name, @"\s+", " ");
        if (name.Length > 40) name = name[..40].TrimEnd();
        return name.Length == 0 ? "Clipboard text" : name;
    }

    // ---------- undo delete ----------

    void UndoDelete()
    {
        if (_lastDeleted.Count == 0) return;
        var items = _lastDeleted;
        _lastDeleted = new();
        _app.Store.Restore(items);
        if (IsVisible) RefreshList(items[0]);
        Toast.Show(items.Count == 1 ? "Restored" : $"Restored {items.Count} items", Area);
    }

    // ---------- plain text for chosen apps ----------

    string? TargetExeName => _target == IntPtr.Zero ? null
        : SourceApp.ExeOf(_target) is { } p ? Path.GetFileNameWithoutExtension(p).ToLowerInvariant() : null;

    bool TargetWantsPlain() => TargetExeName is { } exe && _app.Settings.PlainTextApps.Contains(exe, StringComparer.OrdinalIgnoreCase);

    void TogglePlainForTarget()
    {
        if (TargetExeName is not { } exe) return;
        var s = _app.Settings.Clone();
        if (s.PlainTextApps.RemoveAll(a => a.Equals(exe, StringComparison.OrdinalIgnoreCase)) == 0) s.PlainTextApps.Add(exe);
        _app.SaveSettings(s);
        Toast.Show(TargetWantsPlain() ? "Pastes here will be plain text" : "Pastes here keep formatting", Area);
    }

    // ---------- size and position ----------

    void OnDragArea(object sender, MouseButtonEventArgs e)
    {
        // Only empty space drags: chips and buttons keep working.
        if (e.OriginalSource is DependencyObject d && FindParent<ButtonBase>(d) != null) return;
        try { DragMove(); _moved = true; } catch { }
    }

    void SaveWindowPlacement()
    {
        var s = _app.Settings;
        bool sizeChanged = Math.Abs(s.PopupWidth - Width) > 0.5 || Math.Abs(s.PopupHeight - Height) > 0.5;
        if (!sizeChanged && !_moved) return;
        var c = s.Clone();
        c.PopupWidth = Width;
        c.PopupHeight = Height;
        if (_moved) { c.PopupLeft = Left; c.PopupTop = Top; }
        c.Save();
        _app.ReplaceSettingsQuietly(c);
    }

    void ResetPlacement()
    {
        var c = _app.Settings.Clone();
        c.PopupWidth = 440; c.PopupHeight = 580; c.PopupLeft = null; c.PopupTop = null;
        c.Save();
        _app.ReplaceSettingsQuietly(c);
        Width = 440; Height = 580;
        PlaceWindow(center: _target == IntPtr.Zero);
    }

    // ---------- image editor ----------

    void OpenImageEditor(ClipItem item)
    {
        if (item.ImagePath == null) return;
        BitmapSource img;
        try { img = ImageEdit.Load(item.ImagePath); }
        catch { Toast.Show("Couldn't open the image", Area); return; }
        ResetModes();
        _mode = PopupMode.ImageEdit;
        _imgItem = item;
        _imgCurrent = img;
        ShowEditImage();
        List.Visibility = Visibility.Hidden;
        EmptyState.Visibility = Visibility.Collapsed;
        ImageEditorPane.Visibility = Visibility.Visible;
        OnImgToolChanged(this, new RoutedEventArgs());
    }

    void ShowEditImage()
    {
        EditImage.Source = _imgCurrent;
        ImgSelection.Visibility = Visibility.Collapsed;
        if (_imgCurrent != null) ImgInfo.Text = $"{_imgCurrent.PixelWidth} × {_imgCurrent.PixelHeight}" + (_imgHistory.Count > 0 ? " · edited" : "");
    }

    void ApplyImg(BitmapSource next)
    {
        if (_imgCurrent != null) _imgHistory.Add(_imgCurrent);
        _imgCurrent = next;
        ShowEditImage();
    }

    void ImgRotate() { if (_imgCurrent != null) ApplyImg(ImageEdit.Rotate(_imgCurrent)); }

    void ImgUndo()
    {
        if (_imgHistory.Count == 0) return;
        _imgCurrent = _imgHistory[^1];
        _imgHistory.RemoveAt(_imgHistory.Count - 1);
        ShowEditImage();
    }

    void OnImgToolChanged(object sender, RoutedEventArgs e)
    {
        if (ImgHint == null) return;
        ImgHint.Text = ToolBlur.IsChecked == true
            ? "Drag over anything private to blur it"
            : "Drag over the image to crop it";
    }

    void OnImgRotate(object sender, RoutedEventArgs e) => ImgRotate();
    void OnImgUndo(object sender, RoutedEventArgs e) => ImgUndo();
    void OnImgCancel(object sender, RoutedEventArgs e) { ResetModes(); SearchBox.Focus(); }
    void OnImgSave(object sender, RoutedEventArgs e) => SaveImageEdit(false);
    void OnImgSavePaste(object sender, RoutedEventArgs e) => SaveImageEdit(true);

    void OnImgMouseDown(object sender, MouseButtonEventArgs e)
    {
        _imgDragStart = e.GetPosition(EditImage);
        EditImage.CaptureMouse();
        UpdateSelection(_imgDragStart.Value);
        ImgSelection.Visibility = Visibility.Visible;
        e.Handled = true;
    }

    void OnImgMouseMove(object sender, MouseEventArgs e)
    {
        if (_imgDragStart != null) UpdateSelection(e.GetPosition(EditImage));
    }

    void UpdateSelection(Point p)
    {
        if (_imgDragStart is not { } a) return;
        double x = Math.Clamp(Math.Min(a.X, p.X), 0, EditImage.ActualWidth), y = Math.Clamp(Math.Min(a.Y, p.Y), 0, EditImage.ActualHeight);
        double r = Math.Clamp(Math.Max(a.X, p.X), 0, EditImage.ActualWidth), b = Math.Clamp(Math.Max(a.Y, p.Y), 0, EditImage.ActualHeight);
        Canvas.SetLeft(ImgSelection, x);
        Canvas.SetTop(ImgSelection, y);
        ImgSelection.Width = r - x;
        ImgSelection.Height = b - y;
    }

    void OnImgMouseUp(object sender, MouseButtonEventArgs e)
    {
        if (_imgDragStart is not { } a || _imgCurrent == null) return;
        _imgDragStart = null;
        EditImage.ReleaseMouseCapture();
        ImgSelection.Visibility = Visibility.Collapsed;

        // Screen units → image pixels.
        var p = e.GetPosition(EditImage);
        double sx = _imgCurrent.PixelWidth / EditImage.ActualWidth, sy = _imgCurrent.PixelHeight / EditImage.ActualHeight;
        if (ImageEdit.Clamp(_imgCurrent, a.X * sx, a.Y * sy, p.X * sx, p.Y * sy) is not { } rect) return;
        ApplyImg(ToolBlur.IsChecked == true ? ImageEdit.Pixelate(_imgCurrent, rect) : ImageEdit.Crop(_imgCurrent, rect));
    }

    void SaveImageEdit(bool paste)
    {
        var item = _imgItem;
        if (item == null || _imgCurrent == null) return;
        if (_imgHistory.Count > 0)
        {
            try { _app.Store.UpdateImage(item, _imgCurrent); }
            catch { Toast.Show("Couldn't save the image", Area); return; }
        }
        ResetModes();
        SearchBox.Focus();
        if (paste) Paste(item, PasteMode.Normal);
        else { RefreshList(item); Toast.Show("Saved", Area); }
    }

    // ---------- right-click menu ----------

    static ClipItem? MenuItemOf(object sender) => (sender as FrameworkElement)?.DataContext as ClipItem;

    void Menu_Paste(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) Paste(i, PasteMode.Normal); }
    void Menu_PastePlain(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) Paste(i, PasteMode.Plain); }
    void Menu_Copy(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) Paste(i, PasteMode.CopyOnly); }
    void Menu_Pin(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) TogglePin(i); }
    void Menu_Edit(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) OpenEditor(i); }
    void Menu_Delete(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) { List.SelectedItem = i; Delete(i); } }

    void Menu_Preview(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is not { } i) return;
        List.SelectedItem = i;
        if (_mode != PopupMode.Preview) TogglePreview();
    }

    void Menu_Action(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) DoQuickAction(i); }
    void Menu_SaveFile(object sender, RoutedEventArgs e) { if (MenuItemOf(sender) is { } i) SaveToFile(i); }

    void Menu_OpenQr(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is not { QrIsLink: true } i) return;
        try { Process.Start(new ProcessStartInfo(i.QrText!) { UseShellExecute = true }); } catch { return; }
        Hide();
    }

    void Menu_CopyQr(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is { HasQr: true } i) Paste(i, PasteMode.CopyOnly, i.QrText!);
    }

    void Menu_CopyImageText(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is { HasOcrText: true } i) Paste(i, PasteMode.CopyOnly, i.OcrText!.Trim());
    }

    void Menu_ColorAs(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is not { } i) return;
        var text = (string)((MenuItem)sender).Tag switch { "rgb" => i.ColorRgb, "hsl" => i.ColorHsl, _ => i.ColorHex };
        if (text != null) Paste(i, PasteMode.CopyOnly, text);
    }

    void Menu_Transform(object sender, RoutedEventArgs e)
    {
        if (MenuItemOf(sender) is not { Text: { } text } item) return;
        Paste(item, PasteMode.Normal, TextTools.Apply((string)((MenuItem)sender).Tag, text));
    }

    // ---------- footer menu ----------

    void OnMenuClick(object sender, RoutedEventArgs e)
    {
        var settings = new MenuItem { Header = "Settings…" };
        settings.Click += (_, _) => { Hide(); _app.ShowSettings(); };

        var clear = new MenuItem { Header = "Clear history (keeps pinned)" };
        clear.Click += (_, _) => ClearHistory();

        var folder = new MenuItem { Header = "Open data folder" };
        folder.Click += (_, _) => Process.Start("explorer.exe", ClipStore.Dir);

        var quit = new MenuItem { Header = "Quit ClipVault" };
        quit.Click += (_, _) => Application.Current.Shutdown();

        var compact = new MenuItem { Header = "Compact list", IsCheckable = true, IsChecked = _app.Settings.Compact };
        compact.Click += (_, _) =>
        {
            var c = _app.Settings.Clone();
            c.Compact = !c.Compact;
            _app.SaveSettings(c);
            RefreshList(Selected);
        };

        var menu = new ContextMenu { PlacementTarget = (UIElement)sender, Placement = PlacementMode.Top };
        if (Updater.Ready is { } ready)
        {
            var update = new MenuItem { Header = $"Restart to update to {ready.Major}.{ready.Minor}.{ready.Build}", FontWeight = FontWeights.SemiBold };
            update.Click += (_, _) => Updater.ApplyAndRestart();
            menu.Items.Add(update);
            menu.Items.Add(new Separator());
        }
        menu.Items.Add(compact);
        if (_target != IntPtr.Zero && SourceApp.ExeOf(_target) is { } targetPath)
        {
            var plain = new MenuItem
            {
                Header = $"Always paste plain text into {SourceApp.NameOf(targetPath)}",
                IsCheckable = true, IsChecked = TargetWantsPlain(),
            };
            plain.Click += (_, _) => TogglePlainForTarget();
            menu.Items.Add(plain);
        }
        var reset = new MenuItem { Header = "Reset popup size and position" };
        reset.Click += (_, _) => ResetPlacement();
        menu.Items.Add(reset);
        menu.Items.Add(new Separator());
        menu.Items.Add(settings);
        menu.Items.Add(clear);
        menu.Items.Add(folder);
        menu.Items.Add(new Separator());
        menu.Items.Add(quit);
        menu.IsOpen = true;
    }

    void ClearHistory()
    {
        _keepOpen = true;
        var answer = MessageBox.Show(this, "Delete all unpinned clipboard history? This can't be undone.",
            "ClipVault", MessageBoxButton.OKCancel, MessageBoxImage.Warning);
        _keepOpen = false;
        if (answer == MessageBoxResult.OK)
        {
            _lastDeleted = _app.Store.ClearUnpinned();
            RefreshList(null);
            Toast.Show($"Cleared {_lastDeleted.Count} items", Area, "Undo", UndoDelete);
        }
        Activate();
        SearchBox.Focus();
    }
}
