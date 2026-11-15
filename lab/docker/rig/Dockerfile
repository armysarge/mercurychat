# Rig-control lab image: the M0 lab image plus Hamlib's rigctld/rigctl.
# Used by run-rig-lab.sh with Hamlib's dummy rig (model 1) only; nothing
# here talks to a real transceiver.
FROM mercurychat-lab:m0
RUN apt-get update \
 && apt-get install -y --no-install-recommends libhamlib-utils \
 && rm -rf /var/lib/apt/lists/*
