<script setup lang="ts">
// The session menu (M3c B1, B3, B6): Propose QSY, Request SNR, Request
// station info and Send my email. A control for a capability the peer did
// not list is disabled with "not supported by this station"; pressing it
// never sends anything. The email goes only after a confirmation, once.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import QsyPropose from "./QsyPropose.vue";
import { CAP_INFO, CAP_QSY, extras, peerHas, qsy, requestInfo, sendEmail } from "../extras";
import { activeProfile, session } from "../store";

const { t } = useI18n();
const proposing = ref(false);
const confirmingEmail = ref(false);
const emailSent = ref(false);

const unsupported = computed(() => t("info.refused.unsupported"));
const qsyOk = computed(() => peerHas(CAP_QSY));
const infoOk = computed(() => peerHas(CAP_INFO));
const email = computed(() => activeProfile.value?.email?.trim() ?? "");

async function confirmEmail() {
  confirmingEmail.value = false;
  emailSent.value = await sendEmail();
}

const info = computed(() => (extras.peerInfo && extras.peerInfo.peer === session.value?.peer ? extras.peerInfo : null));
const STATION_KEYS = ["name", "qth", "locator", "rig", "power", "antenna", "about", "email"] as const;
const stationLines = computed(() => {
  const s = info.value?.station;
  if (!s) return [];
  return STATION_KEYS.filter((k) => s[k]).map((k) => ({ key: k, value: s[k] as string }));
});
const reasonText = computed(() => {
  const r = info.value?.reason;
  if (r === "rate") return t("info.reason.rate");
  if (r === "no-reading") return t("info.reason.noReading");
  return "";
});
/** The Station information label of a shared field (the locator is the profile's grid). */
const fieldOf = (k: string) => (k === "locator" ? "grid" : k);
const stationLabel = (k: string) => t(`stationInfo.${fieldOf(k)}`);
const sourceText = (source: string) => (source === "telemetry" ? t("info.source.telemetry") : t("info.source.sn"));
</script>

<template>
  <div v-if="session" class="stack session-menu">
    <div class="actions">
      <button class="qsy-propose" :disabled="!qsyOk || !!qsy" :title="qsyOk ? (qsy ? t('qsy.pending') : '') : unsupported" @click="proposing = !proposing">{{ t("qsy.propose") }}</button>
      <button class="info-snr" :disabled="!infoOk" :title="infoOk ? '' : unsupported" @click="requestInfo('snr')">{{ t("info.requestSnr") }}</button>
      <button class="info-station" :disabled="!infoOk" :title="infoOk ? '' : unsupported" @click="requestInfo('station')">{{ t("info.requestStation") }}</button>
      <button class="info-email" :disabled="!infoOk || !email" :title="!infoOk ? unsupported : email ? '' : t('info.noEmail')" @click="confirmingEmail = true">{{ t("info.sendEmail") }}</button>
    </div>
    <p v-if="!qsyOk || !infoOk" class="muted small unsupported-note">{{ t("info.olderPeer", { peer: session.peer }) }}</p>
    <div v-if="confirmingEmail" class="confirm stack" role="alertdialog" :aria-label="t('info.sendEmail')">
      <p class="small">{{ t("info.confirmEmail", { email, peer: session.peer }) }}</p>
      <div class="row">
        <button class="primary info-email-confirm" @click="confirmEmail">{{ t("info.sendEmailConfirm") }}</button>
        <button @click="confirmingEmail = false">{{ t("settings.cancel") }}</button>
      </div>
    </div>
    <p v-if="emailSent" class="ok small" role="status">{{ t("info.emailSent", { peer: session.peer }) }}</p>
    <QsyPropose v-if="proposing && qsyOk" @done="proposing = false" />

    <div v-if="info" class="peer-info stack small" role="status">
      <p v-if="info.snr" class="peer-snr">
        {{ t("info.snrReport", { peer: info.peer, db: (info.snr.db >= 0 ? "+" : "−") + Math.abs(info.snr.db).toFixed(1), source: sourceText(info.snr.source), age: info.snr.ageS }) }}
      </p>
      <dl v-if="stationLines.length" class="peer-station">
        <template v-for="l in stationLines" :key="l.key">
          <dt class="muted">{{ stationLabel(l.key) }}</dt>
          <dd>{{ l.value }}</dd>
        </template>
      </dl>
      <p v-if="reasonText" class="muted">{{ reasonText }}</p>
    </div>
    <p v-if="extras.error && !qsy" class="err small" role="alert">{{ extras.error }}</p>
  </div>
</template>

<style scoped>
.actions { display: flex; flex-wrap: wrap; gap: 6px; }
.confirm { border: 1px solid var(--border-strong); border-radius: 8px; padding: 6px 8px; }
.peer-station { display: grid; grid-template-columns: auto 1fr; gap: 2px 8px; margin: 0; }
.peer-station dd { margin: 0; overflow-wrap: anywhere; }
</style>
