using System;
using System.Linq;
using System.Numerics;
using System.Threading.Tasks;
using Eto.Drawing;
using Eto.Forms;
using OpenTabletDriver.Desktop.Interop;
using OpenTabletDriver.Desktop.Profiles;
using OpenTabletDriver.Plugin.Platform.Display;
using OpenTabletDriver.UX.Controls.Generic;
using OpenTabletDriver.UX.Controls.Output.Area;
using OpenTabletDriver.UX.Controls.Utilities;
using OpenTabletDriver.UX.Windows;

namespace OpenTabletDriver.UX.Controls.Output
{
    public class AbsoluteModeEditor : Panel
    {
        public AbsoluteModeEditor()
        {
            this.Content = new StackLayout
            {
                HorizontalContentAlignment = HorizontalAlignment.Stretch,
                Items =
                {
                    new StackLayoutItem
                    {
                        Expand = true,
                        Control = new Group
                        {
                            Text = "Display",
                            Content = displayAreaEditor = new DisplayAreaEditor(() => HandleAspectRatioLock(displayAreaEditor, EventArgs.Empty))
                            {
                                InvalidForegroundError = "Invalid display area.",
                                Unit = "px"
                            }
                        }
                    },
                    new StackLayoutItem
                    {
                        Expand = true,
                        Control = new Group
                        {
                            Text = "Tablet",
                            Content = tabletAreaEditor = new TabletAreaEditor
                            {
                                InvalidBackgroundError = "No tablet detected.",
                                InvalidForegroundError = "Invalid tablet area.",
                                Unit = "mm"
                            }
                        }
                    }
                }
            };

            // Both dimensions change as one operation; no intermediate aspect correction.
            var tabletResize = tabletAreaEditor.ContextMenu.Items.GetSubmenu("Resize");
            tabletResize.Items.Clear();
            tabletResize.Items.AddRange(new Command[] {
                new ActionCommand { MenuText = "Full area", Action = () => ResizeTabletArea(false) },
                new ActionCommand { MenuText = "Quarter area", Action = () => ResizeTabletArea(true) }
            });

            displayAreaEditor.AreaBinding.Bind(SettingsBinding.Child(c => c.Display));
            displayAreaEditor.LockToUsableAreaBinding.Bind(App.Current, c => c.Settings.LockUsableAreaDisplay);

            tabletAreaEditor.AreaBinding.Bind(SettingsBinding.Child(c => c.Tablet));
            tabletAreaEditor.LockToUsableAreaBinding.Bind(App.Current, c => c.Settings.LockUsableAreaTablet);

            tabletAreaEditor.LockAspectRatioBinding.Bind(SettingsBinding.Child(c => c.LockAspectRatio));
            tabletAreaEditor.AreaClippingBinding.Bind(SettingsBinding.Child(c => c.EnableClipping));
            tabletAreaEditor.IgnoreOutsideAreaBinding.Bind(SettingsBinding.Child(c => c.EnableAreaLimiting));

            displayWidth = SettingsBinding.Child(c => c.Display.Width);
            displayHeight = SettingsBinding.Child(c => c.Display.Height);
            var displayX = SettingsBinding.Child(c => c.Display.X);
            var displayY = SettingsBinding.Child(c => c.Display.Y);
            displayWidth.DataValueChanged += HandleDisplayAreaConstraint;
            displayHeight.DataValueChanged += HandleDisplayAreaConstraint;
            displayX.DataValueChanged += HandleDisplayAreaConstraint;
            displayY.DataValueChanged += HandleDisplayAreaConstraint;

            tabletWidth = SettingsBinding.Child(c => c.Tablet.Width);
            tabletHeight = SettingsBinding.Child(c => c.Tablet.Height);
            var tabletX = SettingsBinding.Child(c => c.Tablet.X);
            var tabletY = SettingsBinding.Child(c => c.Tablet.Y);
            tabletWidth.DataValueChanged += HandleTabletAreaConstraint;
            tabletHeight.DataValueChanged += HandleTabletAreaConstraint;
            tabletX.DataValueChanged += HandleTabletAreaConstraint;
            tabletY.DataValueChanged += HandleTabletAreaConstraint;

            tabletAreaEditor.LockAspectRatioChanged += HookAspectRatioLock;

            tabletAreaEditor.LockToUsableAreaChanged += HandleTabletAreaConstraint;
            displayAreaEditor.LockToUsableAreaChanged += HandleDisplayAreaConstraint;

            HookAspectRatioLock(tabletAreaEditor, EventArgs.Empty);
        }

        internal DisplayAreaEditor displayAreaEditor;
        internal TabletAreaEditor tabletAreaEditor;

        private bool arLockHooked;
        private bool handlingArLock;
        private bool handlingForcedArConstraint;
        private bool handlingSettingsChanging;
        private DirectBinding<float> displayWidth;
        private DirectBinding<float> displayHeight;
        private DirectBinding<float> tabletWidth;
        private DirectBinding<float> tabletHeight;

        private AbsoluteModeSettings settings;
        public AbsoluteModeSettings Settings
        {
            set
            {
                this.settings = value;
                this.OnSettingsChanged();
            }
            get => this.settings;
        }

        public event EventHandler<EventArgs> SettingsChanged;

        protected virtual void OnSettingsChanged()
        {
            handlingSettingsChanging = true;
            SettingsChanged?.Invoke(this, new EventArgs());
            handlingSettingsChanging = false;
        }

        public BindableBinding<AbsoluteModeEditor, AbsoluteModeSettings> SettingsBinding
        {
            get
            {
                return new BindableBinding<AbsoluteModeEditor, AbsoluteModeSettings>(
                    this,
                    c => c.Settings,
                    (c, v) => c.Settings = v,
                    (c, h) => c.SettingsChanged += h,
                    (c, h) => c.SettingsChanged -= h
                );
            }
        }

        private void HookAspectRatioLock(object sender, EventArgs args)
        {
            lock (this)
            {
                if (Settings?.LockAspectRatio ?? false)
                {
                    if (arLockHooked)
                        return;

                    HandleAspectRatioLock(tabletAreaEditor, EventArgs.Empty);

                    displayWidth.DataValueChanged += HandleAspectRatioLock;
                    displayHeight.DataValueChanged += HandleAspectRatioLock;
                    tabletWidth.DataValueChanged += HandleAspectRatioLock;
                    tabletHeight.DataValueChanged += HandleAspectRatioLock;
                    arLockHooked = true;
                }
                else
                {
                    if (!arLockHooked)
                        return;

                    displayWidth.DataValueChanged -= HandleAspectRatioLock;
                    displayHeight.DataValueChanged -= HandleAspectRatioLock;
                    tabletWidth.DataValueChanged -= HandleAspectRatioLock;
                    tabletHeight.DataValueChanged -= HandleAspectRatioLock;
                    arLockHooked = false;
                }
            }
        }

        private void HandleAspectRatioLock(object sender, EventArgs e)
        {
            if (!tabletAreaEditor.LockAspectRatio) return;
            if (handlingArLock || handlingSettingsChanging || displayAreaEditor.handlingDisplayAreaResize) return;

            if (tabletAreaEditor.Area == null || displayAreaEditor.Area == null
                || tabletAreaEditor.FullAreaBounds.Width <= 0 || tabletAreaEditor.FullAreaBounds.Height <= 0
                || displayAreaEditor.Area.Width <= 0 || displayAreaEditor.Area.Height <= 0) return;

            // Avoids looping
            handlingArLock = true;

            var displayRatio = displayAreaEditor.Area.Width / displayAreaEditor.Area.Height;

            var tryHeightFirst = sender == tabletWidth || sender == displayHeight || sender == tabletAreaEditor || sender == displayAreaEditor && displayRatio <= 1;
            var tryWidthFirst = sender == tabletHeight || sender == displayWidth || sender == displayAreaEditor && displayRatio > 1;

            if (tryHeightFirst)
            {
                var fullHeight = tabletAreaEditor.FullAreaBounds.Height;
                var scaledHeight = displayHeight.DataValue / displayWidth.DataValue * tabletWidth.DataValue;
                if (scaledHeight > fullHeight)
                {
                    tabletHeight.DataValue = fullHeight;
                    tabletWidth.DataValue = displayWidth.DataValue / displayHeight.DataValue * fullHeight;
                }
                else
                {
                    tabletHeight.DataValue = scaledHeight;
                }
            }
            else if (tryWidthFirst)
            {
                var fullWidth = tabletAreaEditor.FullAreaBounds.Width;
                var scaledWidth = displayWidth.DataValue / displayHeight.DataValue * tabletHeight.DataValue;
                if (scaledWidth > fullWidth)
                {
                    tabletWidth.DataValue = fullWidth;
                    tabletHeight.DataValue = displayHeight.DataValue / displayWidth.DataValue * fullWidth;
                }
                else
                {
                    tabletWidth.DataValue = scaledWidth;
                }
            }

            handlingArLock = false;
        }

        private void HandleTabletAreaConstraint(object sender, EventArgs args)
        {
            ForceAreaConstraint(tabletAreaEditor.Display, args);
        }

        private void HandleDisplayAreaConstraint(object sender, EventArgs args)
        {
            ForceAreaConstraint(displayAreaEditor.Display, args);
        }

        private void ForceAreaConstraint(object sender, EventArgs args)
        {
            var display = (AreaDisplay)sender;
            if (!handlingForcedArConstraint && !handlingSettingsChanging && display.LockToUsableArea && display.Area != null)
            {
                handlingForcedArConstraint = true;
                var fullBounds = display.FullAreaBounds;

                if (fullBounds.Width != 0 && fullBounds.Height != 0)
                {
                    if (display.Area.Rotation % 180 == 0)
                    {
                        if (display.Area.Width > fullBounds.Width)
                            display.Area.Width = fullBounds.Width;
                        if (display.Area.Height > fullBounds.Height)
                            display.Area.Height = fullBounds.Height;
                    }

                    var correction = GetOutOfBoundsAmount(display, display.Area.X, display.Area.Y);
                    display.Area.X -= correction.X;
                    display.Area.Y -= correction.Y;
                }

                handlingForcedArConstraint = false;
            }
        }

        private static Vector2 GetOutOfBoundsAmount(AreaDisplay display, float X, float Y)
        {
            var bounds = display.FullAreaBounds;
            bounds.X = 0;
            bounds.Y = 0;

            var area = display.Area;
            var rect = RectangleF.FromCenter(PointF.Empty, new SizeF(area.Width, area.Height));

            var corners = new PointF[]
            {
                    PointF.Rotate(rect.TopLeft, area.Rotation),
                    PointF.Rotate(rect.TopRight, area.Rotation),
                    PointF.Rotate(rect.BottomRight, area.Rotation),
                    PointF.Rotate(rect.BottomLeft, area.Rotation)
            };

            var pseudoArea = new RectangleF(
                PointF.Min(corners[0], PointF.Min(corners[1], PointF.Min(corners[2], corners[3]))),
                PointF.Max(corners[0], PointF.Max(corners[1], PointF.Max(corners[2], corners[3])))
            );

            pseudoArea.Center += new PointF(X, Y);

            return new Vector2
            {
                X = Math.Max(pseudoArea.Right - bounds.Right - 1, 0) + Math.Min(pseudoArea.Left - bounds.Left, 0),
                Y = Math.Max(pseudoArea.Bottom - bounds.Bottom - 1, 0) + Math.Min(pseudoArea.Top - bounds.Top, 0)
            };
        }

        private void ResizeTabletArea(bool quarter)
        {
            var area = tabletAreaEditor.Area;
            var bounds = tabletAreaEditor.FullAreaBounds;
            if (area == null || bounds.Width <= 0 || bounds.Height <= 0 || displayAreaEditor.Area == null) return;
            var dimensions = new Vector2(bounds.Width, bounds.Height);
            var ratio = displayAreaEditor.Area.Width / displayAreaEditor.Area.Height;
            var size = tabletAreaEditor.LockAspectRatio
                ? GetLargestRectInRotatedRectRatioLocked(dimensions, area.Rotation, ratio)
                : GetLargestRectInRotatedRect(dimensions, area.Rotation);
            handlingArLock = true;
            handlingForcedArConstraint = true;
            try
            {
                area.Width = size.X / (quarter ? 2 : 1);
                area.Height = size.Y / (quarter ? 2 : 1);
                area.X = bounds.Center.X;
                area.Y = bounds.Center.Y;
            }
            finally { handlingArLock = false; handlingForcedArConstraint = false; }
            HandleTabletAreaConstraint(tabletAreaEditor, EventArgs.Empty);
        }

        // RC #5102: maximal free rectangle inside rotated bounds.
        public static Vector2 GetLargestRectInRotatedRect(Vector2 size, float rotation)
        {
            double width = size.X, height = size.Y;
            if (width <= 0 || height <= 0 || !float.IsFinite(rotation)) return Vector2.Zero;
            double angle = rotation * Math.PI / 180;
            double sin = Math.Abs(Math.Sin(angle)), cos = Math.Abs(Math.Cos(angle));
            bool widthIsLonger = width >= height;
            double longer = Math.Max(width, height), shorter = Math.Min(width, height);
            if (shorter <= 2 * sin * cos * longer || Math.Abs(sin - cos) < 1e-10)
            {
                double half = shorter / 2;
                return widthIsLonger ? new Vector2((float)(half / sin), (float)(half / cos))
                    : new Vector2((float)(half / cos), (float)(half / sin));
            }
            double cosTwo = cos * cos - sin * sin;
            return new Vector2((float)((width * cos - height * sin) / cosTwo),
                (float)((height * cos - width * sin) / cosTwo));
        }

        // Same edge intersections and four-decimal rounding as RC #5102,
        // expressed without the infinite slopes at cardinal rotations.
        public static Vector2 GetLargestRectInRotatedRectRatioLocked(Vector2 size, float rotation, float ratio)
        {
            if (size.X <= 0 || size.Y <= 0 || !float.IsFinite(rotation) || !float.IsFinite(ratio) || ratio <= 0) return Vector2.Zero;
            double angle = rotation * Math.PI / 180;
            double sin = Math.Abs(Math.Sin(angle)), cos = Math.Abs(Math.Cos(angle));
            double height = Math.Min(size.X / (ratio * cos + sin), size.Y / (ratio * sin + cos));
            double width = height * ratio;
            if (Math.Abs(width - Math.Round(width)) <= Math.Abs(height - Math.Round(height)))
            {
                width = Math.Round(width, 4);
                height = width / ratio;
            }
            else { height = Math.Round(height, 4); width = height * ratio; }
            return new Vector2((float)width, (float)height);
        }

        public class DisplayAreaEditor : AreaEditor
        {
            public bool handlingDisplayAreaResize;
            private readonly Action triggerAspectRatioLock;
            public DisplayAreaEditor(Action triggerAspectRatioLock)
            {
                this.triggerAspectRatioLock = triggerAspectRatioLock;
                this.ToolTip = "You can right click the area editor to set the area to a display, adjust alignment, or resize the area.";
            }

            protected override void CreateMenu()
            {
                base.CreateMenu();
                var resize = ContextMenu.Items.GetSubmenu("Resize");
                resize.Items.Clear();
                resize.Items.AddRange(new Command[] {
                    new ActionCommand { MenuText = "Full area", Action = () => SetArea(FullAreaBounds.Width, FullAreaBounds.Height, FullAreaBounds.Center.X, FullAreaBounds.Center.Y) },
                    new ActionCommand { MenuText = "Quarter area", Action = () => SetArea(FullAreaBounds.Width / 2, FullAreaBounds.Height / 2, FullAreaBounds.Center.X, FullAreaBounds.Center.Y) }
                });

                base.ContextMenu.Items.AddSeparator();

                var subMenu = base.ContextMenu.Items.GetSubmenu("Set to display");

                var displays = DesktopInterop.VirtualScreen.Displays.ToArray();

                // account for monitor layouts with negative offsets (e.g. Wayland supports this)
                // skip IVirtualScreen's as these tend to be normalized to 0,0, which may confuse these methods
                float xOffset = displays.Where(d => d is not IVirtualScreen).MinBy(d => d.Position.X).Position.X;
                float yOffset = displays.Where(d => d is not IVirtualScreen).MinBy(d => d.Position.Y).Position.Y;

                foreach (var display in displays)
                {
                    subMenu.Items.Add(
                        new ActionCommand
                        {
                            MenuText = display.ToString(),
                            Action = () =>
                            {
                                float x, y;
                                if (display is IVirtualScreen virtualScreen)
                                {
                                    x = virtualScreen.Width / 2;
                                    y = virtualScreen.Height / 2;
                                }
                                else
                                {
                                    virtualScreen = DesktopInterop.VirtualScreen;
                                    x = display.Position.X - xOffset + virtualScreen.Position.X + display.Width / 2;
                                    y = display.Position.Y - yOffset + virtualScreen.Position.Y + display.Height / 2;
                                }
                                SetArea(display.Width, display.Height, x, y);
                            }
                        }
                    );
                }
            }
            private void SetArea(float width, float height, float x, float y)
            {
                if (Area == null || width <= 0 || height <= 0) return;
                handlingDisplayAreaResize = true;
                try { Area.Width = width; Area.Height = height; Area.X = x; Area.Y = y; }
                finally { handlingDisplayAreaResize = false; }
                triggerAspectRatioLock();
            }
        }

        public class TabletAreaEditor : RotationAreaEditor
        {
            public TabletAreaEditor()
            {
                this.ToolTip = "You can right click the area editor to enable aspect ratio locking, adjust alignment, or resize the area.";
            }

            private BooleanCommand lockArCmd, areaClippingCmd, ignoreOutsideAreaCmd;
            private bool lockAspectRatio, areaClipping, ignoreOutsideArea;

            public event EventHandler<EventArgs> LockAspectRatioChanged;
            public event EventHandler<EventArgs> AreaClippingChanged;
            public event EventHandler<EventArgs> IgnoreOutsideAreaChanged;

            protected virtual void OnLockAspectRatioChanged() => LockAspectRatioChanged?.Invoke(this, new EventArgs());
            protected virtual void OnAreaClippingChanged() => AreaClippingChanged?.Invoke(this, new EventArgs());
            protected virtual void OnIgnoreOutsideAreaChanged() => IgnoreOutsideAreaChanged?.Invoke(this, new EventArgs());

            public bool LockAspectRatio
            {
                set
                {
                    this.lockAspectRatio = value;
                    this.OnLockAspectRatioChanged();
                }
                get => this.lockAspectRatio;
            }

            public bool AreaClipping
            {
                set
                {
                    this.areaClipping = value;
                    this.OnAreaClippingChanged();
                }
                get => this.areaClipping;
            }

            public bool IgnoreOutsideArea
            {
                set
                {
                    this.ignoreOutsideArea = value;
                    this.OnIgnoreOutsideAreaChanged();
                }
                get => this.ignoreOutsideArea;
            }

            public BindableBinding<TabletAreaEditor, bool> LockAspectRatioBinding
            {
                get
                {
                    return new BindableBinding<TabletAreaEditor, bool>(
                        this,
                        c => c.LockAspectRatio,
                        (c, v) => c.LockAspectRatio = v,
                        (c, h) => c.LockAspectRatioChanged += h,
                        (c, h) => c.LockAspectRatioChanged -= h
                    );
                }
            }

            public BindableBinding<TabletAreaEditor, bool> AreaClippingBinding
            {
                get
                {
                    return new BindableBinding<TabletAreaEditor, bool>(
                        this,
                        c => c.AreaClipping,
                        (c, v) => c.AreaClipping = v,
                        (c, h) => c.AreaClippingChanged += h,
                        (c, h) => c.AreaClippingChanged -= h
                    );
                }
            }

            public BindableBinding<TabletAreaEditor, bool> IgnoreOutsideAreaBinding
            {
                get
                {
                    return new BindableBinding<TabletAreaEditor, bool>(
                        this,
                        c => c.IgnoreOutsideArea,
                        (c, v) => c.IgnoreOutsideArea = v,
                        (c, h) => c.IgnoreOutsideAreaChanged += h,
                        (c, h) => c.IgnoreOutsideAreaChanged -= h
                    );
                }
            }

            protected override void CreateMenu()
            {
                base.CreateMenu();

                base.ContextMenu.Items.AddSeparator();

                lockArCmd = new BooleanCommand
                {
                    MenuText = "Lock aspect ratio"
                };

                areaClippingCmd = new BooleanCommand
                {
                    MenuText = "Clamp input outside area"
                };

                ignoreOutsideAreaCmd = new BooleanCommand
                {
                    MenuText = "Ignore input outside area"
                };

                base.ContextMenu.Items.AddRange(
                    new Command[]
                    {
                        lockArCmd,
                        areaClippingCmd,
                        ignoreOutsideAreaCmd
                    }
                );

                base.ContextMenu.Items.AddSeparator();

                base.ContextMenu.Items.Add(
                    new ActionCommand
                    {
                        MenuText = "Convert area...",
                        Action = async () => await ConvertAreaDialog()
                    }
                );

                lockArCmd.CheckedBinding.Cast<bool>().Bind(LockAspectRatioBinding);
                areaClippingCmd.CheckedBinding.Cast<bool>().Bind(AreaClippingBinding);
                ignoreOutsideAreaCmd.CheckedBinding.Cast<bool>().Bind(IgnoreOutsideAreaBinding);
            }

            private async Task ConvertAreaDialog()
            {
                var converter = new AreaConverterDialog
                {
                    DataContext = base.Area
                };
                await converter.ShowModalAsync(base.ParentWindow);
            }
        }
    }
}
