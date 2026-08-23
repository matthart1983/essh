# Demo host for the README tour.
#
# Two of these run bookworm and one runs bullseye, which is where the
# divergence in the recording comes from: the older host really is behind on
# its OS, OpenSSL and packages. Nothing in the tour is staged — it is collected
# over live SSH sessions against these containers.
ARG BASE=debian:12
FROM ${BASE}

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      openssh-server nginx tmux procps iproute2 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /run/sshd /root/.ssh \
 && chmod 700 /root/.ssh \
 && sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config

# The recording connects with the operator's own key; the script that builds
# these copies it in rather than baking a secret into the image.
COPY authorized_keys /root/.ssh/authorized_keys
RUN chmod 600 /root/.ssh/authorized_keys

EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
