using System.Numerics;
using OpenTabletDriver.Plugin.Tablet;
using OpenTabletDriver.Plugin.Tablet.Touch;
using OpenTabletDriver.Plugin.Tablet.Wheel;

namespace OtdCompat;

// Concrete interface sets are observable to unchanged plugins. A report with
// no tilt or eraser field must not acquire those capabilities in the bridge.
class TiltPenSnapshot : PenSnapshot, ITiltReport { public Vector2 Tilt { get; set; } }
class ProximityPenSnapshot : PenSnapshot, IProximityReport
{
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
sealed class TiltProximityPenSnapshot : TiltPenSnapshot, IProximityReport
{
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
class EraserProximitySnapshot : Report, IProximityReport
{
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
sealed class AuxPenSnapshot : EraserProximitySnapshot, IAuxReport
{
    public bool[] AuxButtons { get; set; } = [];
}
sealed class ToolSnapshot : IToolReport, IEraserReport, IProximityReport
{
    public byte[] Raw { get; set; } = [];
    public ulong Serial { get; set; }
    public uint RawToolID { get; set; }
    public ToolType Tool { get; set; }
    public bool Eraser { get; set; }
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
class MouseSnapshot : IMouseReport
{
    public byte[] Raw { get; set; } = [];
    public Vector2 Position { get; set; }
    public bool[] MouseButtons { get; set; } = [];
    public Vector2 Scroll { get; set; }
}
sealed class ProximityMouseSnapshot : MouseSnapshot, IProximityReport
{
    public bool NearProximity { get; set; }
    public uint HoverDistance { get; set; }
}
sealed class AuxMouseSnapshot : MouseSnapshot, IAuxReport
{
    public bool[] AuxButtons { get; set; } = [];
}
class AbsoluteSnapshot : IAbsoluteAnalogReport
{
    public byte[] Raw { get; set; } = [];
    public uint?[] AnalogPositions { get; set; } = [];
}
sealed class AbsoluteWheelSnapshot : AbsoluteSnapshot, IAbsoluteWheelReport { }
sealed class AuxAbsoluteSnapshot : AbsoluteSnapshot, IAuxReport
{
    public bool[] AuxButtons { get; set; } = [];
}
class RelativeSnapshot : IRelativeAnalogReport
{
    public byte[] Raw { get; set; } = [];
    public int[] AnalogDeltas { get; set; } = [];
}
class RelativeWheelSnapshot : RelativeSnapshot, IRelativeWheelReport { }
sealed class AuxRelativeWheelSnapshot : RelativeWheelSnapshot, IAuxReport
{
    public bool[] AuxButtons { get; set; } = [];
}
sealed class AuxWheelButtonsSnapshot : AuxSnapshot, IWheelButtonReport
{
    public bool[][] WheelButtons { get; set; } = [];
}
sealed class AuxTouchSnapshot : AuxSnapshot, ITouchReport
{
    public TouchPoint[] Touches { get; set; } = [];
}
