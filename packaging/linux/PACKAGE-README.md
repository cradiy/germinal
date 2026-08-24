# Germinal Linux package

This package contains the `germinal` desktop binary, its Freedesktop desktop entry, and the
Germinal application icon.

The binary dynamically links to Fontconfig, FreeType, GLib, and the GStreamer core and base
libraries. A Vulkan-capable, OpenGL-capable, or software WGPU backend must also be available.

For a system-wide installation from the archive, copy its `bin` and `share` directories into a
directory on the system prefix, for example `/usr/local`. Native DEB, RPM, and Arch Linux packages
install the same files under `/usr` and declare the direct runtime library dependencies.

The `linux-musl` archive is built for musl but is not fully static. It requires musl-compatible
Fontconfig, FreeType, GLib, GStreamer, and GStreamer plugins on the destination system. DEB, RPM,
and Arch Linux outputs intentionally remain glibc packages.
