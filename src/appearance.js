const invoke = window.__TAURI__?.core?.invoke;

const controls = {
  lightWidth: document.getElementById("light-width"),
  lightWidthValue: document.getElementById("light-width-value"),
  fontSize: document.getElementById("font-size"),
  fontSizeValue: document.getElementById("font-size-value"),
  fontFamily: document.getElementById("font-family"),
  fontColor: document.getElementById("font-color"),
  fontColorText: document.getElementById("font-color-text"),
  panelColor: document.getElementById("panel-color"),
  panelColorText: document.getElementById("panel-color-text"),
  fontWeight: document.getElementById("font-weight"),
  status: document.getElementById("status"),
  reset: document.getElementById("reset"),
};

let saveTimer = 0;

function readForm() {
  return {
    lightWidth: Number(controls.lightWidth.value),
    labelFontFamily: controls.fontFamily.value.trim(),
    labelFontSize: Number(controls.fontSize.value),
    labelColor: controls.fontColorText.value.trim(),
    labelFontWeight: Number(controls.fontWeight.value),
    panelColor: controls.panelColorText.value.trim(),
  };
}

function writeForm(appearance) {
  controls.lightWidth.value = appearance.lightWidth;
  controls.fontSize.value = appearance.labelFontSize;
  controls.fontFamily.value = appearance.labelFontFamily;
  controls.fontColor.value = appearance.labelColor;
  controls.fontColorText.value = appearance.labelColor;
  controls.fontWeight.value = String(appearance.labelFontWeight);
  controls.panelColor.value = appearance.panelColor;
  controls.panelColorText.value = appearance.panelColor;
  updateOutputs();
}

function updateOutputs() {
  controls.lightWidthValue.value = `${controls.lightWidth.value} px`;
  controls.fontSizeValue.value = `${controls.fontSize.value} px`;
}

function scheduleSave() {
  updateOutputs();
  window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(save, 120);
}

async function save() {
  try {
    const appearance = await invoke?.("save_appearance", { update: readForm() });
    if (appearance) {
      writeForm(appearance);
      controls.status.textContent = "已保存";
    }
  } catch (error) {
    controls.status.textContent = String(error);
  }
}

controls.lightWidth.addEventListener("input", scheduleSave);
controls.fontSize.addEventListener("input", scheduleSave);
controls.fontFamily.addEventListener("change", scheduleSave);
controls.fontWeight.addEventListener("change", scheduleSave);
controls.fontColor.addEventListener("input", () => {
  controls.fontColorText.value = controls.fontColor.value;
  scheduleSave();
});
controls.fontColorText.addEventListener("change", () => {
  if (/^#[0-9a-f]{6}$/i.test(controls.fontColorText.value)) {
    controls.fontColor.value = controls.fontColorText.value;
  }
  scheduleSave();
});
controls.panelColor.addEventListener("input", () => {
  controls.panelColorText.value = controls.panelColor.value;
  scheduleSave();
});
controls.panelColorText.addEventListener("change", () => {
  if (/^#[0-9a-f]{6}$/i.test(controls.panelColorText.value)) {
    controls.panelColor.value = controls.panelColorText.value;
  }
  scheduleSave();
});
controls.reset.addEventListener("click", () => {
  writeForm({
    lightWidth: 66,
    labelFontFamily: "Segoe UI",
    labelFontSize: 12,
    labelColor: "#f5f5f5",
    labelFontWeight: 700,
    panelColor: "#171a1f",
  });
  save();
});

async function initialize() {
  try {
    const appearance = await invoke?.("get_appearance");
    if (appearance) {
      writeForm(appearance);
    }
  } catch (error) {
    controls.status.textContent = String(error);
  }
}

initialize();
