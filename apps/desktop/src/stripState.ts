// Whether the station panel (the former strip) is open: shared by the
// panel's own close button and the header's station pill (M5 Task 11b), and
// kept between runs. Closed by default: the owner asked for no bar under the
// header (23 Nov), so a new key, and the old "open" choice is not carried.
import { ref, watch } from "vue";

const KEY = "mc.stationPanelOpen";

export const stripOpen = ref(
  (() => {
    try {
      return localStorage.getItem(KEY) === "1";
    } catch {
      return false;
    }
  })(),
);
watch(stripOpen, (v) => {
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    // Storage may be unavailable: the choice then lasts for this run only.
  }
});
