FROM scratch
LABEL maintainer="Ioannis Anagnostakis"
LABEL org.opencontainers.image.description="Minimal bootstrappable Slackware64-current (~54MB pull): pkgtools + slacker with resolve-stock. md5-bootstrap: install gnupg2, then 'slacker update gpg' for full GPG verification."
LABEL org.opencontainers.image.source="https://github.com/rizitis/slackware64-current-ci"
LABEL org.opencontainers.image.url="https://github.com/rizitis/slacker"
LABEL org.opencontainers.image.documentation="https://forge.slackware.nl/rizitis/slacker/wiki/Docker"
LABEL org.opencontainers.image.licenses="Apache-2.0"
ADD slackware-rootfs.tar.gz /
ENV PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CMD ["/usr/bin/bash"]
