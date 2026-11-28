Name:           ssd-sentinel
Version:        1.11.0
Release:        1%{?dist}
Summary:        NVMe/SATA/HDD storage diagnostics, io_uring benchmark, surface repair, and file recovery

License:        MIT
URL:            https://github.com/yuefdev/ssd-sentinel
Requires:       glibc
Recommends:     smartmontools, polkit, udisks2

%description
Pure native Rust desktop and CLI tool for NVMe/SATA/HDD SMART telemetry,
io_uring / O_DIRECT storage benchmarking, surface verification and repair,
Linux storage policy tuning, developer workspace cleanup, and read-only
file recovery from XDG Trash, snapshots, and raw storage.

%install
mkdir -p %{buildroot}%{_bindir} \
         %{buildroot}%{_datadir}/applications \
         %{buildroot}%{_datadir}/metainfo \
         %{buildroot}%{_datadir}/polkit-1/actions \
         %{buildroot}%{_datadir}/icons/hicolor/32x32/apps \
         %{buildroot}%{_datadir}/icons/hicolor/128x128/apps \
         %{buildroot}%{_datadir}/icons/hicolor/256x256/apps

install -m755 %{_sourcedir}/ssd-sentinel %{buildroot}%{_bindir}/ssd-sentinel
install -m644 %{_sourcedir}/com.ssdsentinel.desktop.desktop %{buildroot}%{_datadir}/applications/com.ssdsentinel.desktop.desktop
install -m644 %{_sourcedir}/com.ssdsentinel.desktop.metainfo.xml %{buildroot}%{_datadir}/metainfo/com.ssdsentinel.desktop.metainfo.xml
install -m644 %{_sourcedir}/com.ssdsentinel.desktop.policy %{buildroot}%{_datadir}/polkit-1/actions/com.ssdsentinel.desktop.policy
install -m644 %{_sourcedir}/icon-32.png %{buildroot}%{_datadir}/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png
install -m644 %{_sourcedir}/icon-128.png %{buildroot}%{_datadir}/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png
install -m644 %{_sourcedir}/icon-256.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png

%files
%{_bindir}/ssd-sentinel
%{_datadir}/applications/com.ssdsentinel.desktop.desktop
%{_datadir}/metainfo/com.ssdsentinel.desktop.metainfo.xml
%{_datadir}/polkit-1/actions/com.ssdsentinel.desktop.policy
%{_datadir}/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png
%{_datadir}/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png
%{_datadir}/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png
