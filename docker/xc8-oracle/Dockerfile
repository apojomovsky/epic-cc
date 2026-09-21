# The XC8 test-oracle image (epic-cc#527).
#
# Separate from the dev image because XC8 is licence-gated: it must not
# ride in an image `release`/`ci` derive from. Build context is this
# directory; `make oracle-image` stages the gitignored installer next to
# this file first. DFPs are not bundled with the installer and come from
# Microchip's public pack server.

FROM ubuntu:22.04

ARG XC8_VERSION=4.00
ARG PIC16FXXX_DFP_VERSION=1.7.162
ARG PIC18FXXXX_DFP_VERSION=1.7.171
ARG PIC12_16F1XXX_DFP_VERSION=1.9.258

ENV XC8_INSTALL_DIR=/opt/microchip/xc8/v${XC8_VERSION}
# XC8's bin is deliberately NOT prepended to PATH: the `/usr/local/bin/xc8-cc`
# wrapper below is the supported entry point (it supplies `-mdfp`), and having
# the real binary earlier on PATH would shadow it.
ENV PATH="${PATH}:/usr/local/bin"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

# `--mode unattended` accepts the EULA on the builder's behalf; that is the
# operator's acceptance, which is why the installer is user-supplied rather
# than fetched here. The guard is the same one epic-hal uses: a truncated or
# placeholder file would otherwise produce a confusing failure much later.
COPY xc8-installer.run /tmp/installer.run
RUN set -eu; \
    size=$(stat -c%s /tmp/installer.run); \
    if [ "$size" -lt 10000000 ]; then \
        echo "error: xc8-installer.run is only ${size} bytes; expected the" \
             "multi-megabyte XC8 v${XC8_VERSION} Linux installer (.run)." \
             "See vendor/README.md." >&2; \
        exit 1; \
    fi; \
    chmod +x /tmp/installer.run; \
    /tmp/installer.run --mode unattended --unattendedmodeui none \
        --prefix "${XC8_INSTALL_DIR}"; \
    rm /tmp/installer.run; \
    cd "${XC8_INSTALL_DIR}" && rm -rf \
        avr docs \
        "Uninstall MPLAB XC8 C Compiler.desktop" \
        Uninstall-xc8-v4.00 Uninstall-xc8-v4.dat

RUN set -eu; \
    for pack_ver in \
        "Microchip.PIC16Fxxx_DFP:${PIC16FXXX_DFP_VERSION}" \
        "Microchip.PIC18Fxxxx_DFP:${PIC18FXXXX_DFP_VERSION}" \
        "Microchip.PIC12-16F1xxx_DFP:${PIC12_16F1XXX_DFP_VERSION}"; \
    do \
        pack="${pack_ver%%:*}"; \
        ver="${pack_ver##*:}"; \
        dest="${XC8_INSTALL_DIR}/pic/packs/${pack}"; \
        mkdir -p "$dest"; \
        curl -fsSL -o "/tmp/${pack}.atpack" \
            "https://packs.download.microchip.com/${pack}.${ver}.atpack"; \
        unzip -q "/tmp/${pack}.atpack" -d "$dest"; \
        rm "/tmp/${pack}.atpack"; \
    done

# `-mdfp` must name a pack's `xc8` subdirectory; XC8 v4.00 does not
# discover device files on its own, and the wrong suffix fails with
# `error: (2104) no device-support files found`. XC8 has no environment
# variable for this, so a wrapper supplies it: callers then need only
# `-mcpu=...`, and cannot get the suffix wrong. `XC8C_DFP` overrides the
# family (default PIC18; PIC14 callers pass the PIC16Fxxx pack's `xc8`).
ENV PIC8_XC8_DFP=${XC8_INSTALL_DIR}/pic/packs/Microchip.PIC18Fxxxx_DFP/xc8
RUN set -eu; \
    printf '#!/bin/sh\nexec %s/bin/xc8-cc -mdfp="${XC8C_DFP:-$PIC8_XC8_DFP}" "$@"\n' \
        "${XC8_INSTALL_DIR}" > /usr/local/bin/xc8-cc \
    && chmod +x /usr/local/bin/xc8-cc

# A smoke check at build time: the image is worthless if xc8-cc cannot
# actually produce a PIC18 object, and this fails the build rather than the
# first user of the oracle. Goes through the wrapper, so it also proves the
# default `-mdfp` resolves.
RUN set -eu; \
    printf 'volatile unsigned char out;\nvoid main(void){ out = 42; }\n' > /tmp/smoke.c; \
    xc8-cc -mcpu=18f4550 -O2 -c /tmp/smoke.c -o /tmp/smoke.p1; \
    test -s /tmp/smoke.p1; \
    rm -f /tmp/smoke.c /tmp/smoke.p1 /tmp/smoke.d
