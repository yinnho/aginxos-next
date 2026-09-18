# 81voltd — IMS Data QMI server (service 770)

Vendored protocol (`imsd.qmi`, `qmi_imsd.c/h`) and license from
[81voltd](https://gitlab.postmarketos.org/modem/81voltd)
(Richard Acayan, GPL-2.0-or-later).

Upstream 81voltd talks ModemManager over D-Bus (glib). AginxOS L0 has
neither, so `81voltd.c` is a libqrtr port of `qvd-server.c`: publish
QRTR service 770, answer START/STOP CONNECTION, treat everything else
as no-op. The data-path backend is a WDS QMI client (the same messages
81voltd asks ModemManager to send), not MM.

Binary: `/usr/bin/81voltd` (enchilada bake). Log: `/var/81voltd.log`.
