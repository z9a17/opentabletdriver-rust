using System.Collections.Generic;
using System.Diagnostics.CodeAnalysis;

namespace OpenTabletDriver.Plugin.Tablet
{
    public class TabletReference
    {
        [SetsRequiredMembers]
        public TabletReference()
        {
            Properties = null!; // Preserve the original parameterless default.
        }

        [SetsRequiredMembers]
        public TabletReference(
            TabletConfiguration properties,
            IEnumerable<DeviceIdentifier> identifiers
        )
        {
            this.Properties = properties;
            this.Identifiers = identifiers;
        }

        public required TabletConfiguration Properties { set; get; }
        public IEnumerable<DeviceIdentifier> Identifiers { set; get; } = [];
    }
}
