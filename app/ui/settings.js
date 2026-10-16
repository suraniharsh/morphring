// The settings window: default output, two toggles, the preset list and the
// config folder. Every change saves itself after a short pause, so there is
// no Save button to forget about.
//
// Backend contract (window.__TAURI__, no bundler):
//   commands   settings_get              -> the app settings (camelCase JSON)
//                                           plus configDir (string or null)
//              settings_set { settings } -> validates and saves them, or
//                                           rejects with a readable message
//              presets_list              -> [{ id, name, target }]
//   plugins    dialog.open               -> the custom folder picker
//              opener.revealItemInDir    -> "Open folder"
"use strict";

(function () {
  const SAVE_DELAY_MS = 400;
  const tauri = window.__TAURI__;
  const invoke = tauri.core.invoke;

  const form = document.getElementById("settings");
  const subfolderRow = document.getElementById("subfolder-row");
  const subfolderName = document.getElementById("subfolder-name");
  const customRow = document.getElementById("custom-row");
  const customFolder = document.getElementById("custom-folder");
  const chooseFolder = document.getElementById("choose-folder");
  const keepFileDates = document.getElementById("keep-file-dates");
  const keepHistory = document.getElementById("keep-history");
  const presetList = document.getElementById("presets");
  const configDirText = document.getElementById("config-dir");
  const openFolder = document.getElementById("open-folder");
  const status = document.getElementById("status");

  // The last settings the backend handed us. Fields this page does not edit
  // ride along untouched, so a newer backend's extras survive a save.
  let settings = null;
  let configDir = null;
  let saveTimer = null;
  let saving = Promise.resolve();

  function showStatus(text, isError) {
    status.textContent = text;
    status.classList.toggle("error", Boolean(isError));
  }

  function location() {
    const checked = form.querySelector('input[name="location"]:checked');
    return checked ? checked.value : "SameFolder";
  }

  function showLocationRows() {
    const where = location();
    subfolderRow.hidden = where !== "Subfolder";
    customRow.hidden = where !== "CustomFolder";
  }

  function render() {
    const out = settings.defaultOutput || {};
    const radio = form.querySelector(
      'input[name="location"][value="' + (out.location || "SameFolder") + '"]',
    );
    (radio || form.querySelector('input[name="location"]')).checked = true;
    subfolderName.value = out.subfolderName || "";
    customFolder.value = out.customFolder || "";
    keepFileDates.checked = Boolean(settings.keepFileDates);
    keepHistory.checked = Boolean(settings.keepHistory);
    showLocationRows();

    configDirText.textContent = configDir || "No config folder on this system";
    openFolder.disabled = !configDir;
  }

  function collect() {
    const next = Object.assign({}, settings);
    delete next.configDir;
    next.keepFileDates = keepFileDates.checked;
    next.keepHistory = keepHistory.checked;
    next.defaultOutput = Object.assign({}, settings.defaultOutput, {
      location: location(),
      subfolderName: subfolderName.value,
      customFolder: customFolder.value,
    });
    return next;
  }

  function markInvalid(message) {
    const where = location();
    subfolderName.setAttribute("aria-invalid", String(Boolean(message) && where === "Subfolder"));
    customFolder.setAttribute("aria-invalid", String(Boolean(message) && where === "CustomFolder"));
  }

  // Saves are chained so two quick edits can never land out of order. The
  // backend is the judge of validity; we just pass on its verdict.
  function save() {
    const next = collect();
    saving = saving.then(async function () {
      try {
        await invoke("settings_set", { settings: next });
        settings = next;
        markInvalid(null);
        showStatus("Saved", false);
      } catch (err) {
        markInvalid(String(err));
        showStatus(String(err), true);
      }
    });
    return saving;
  }

  function scheduleSave() {
    showLocationRows();
    clearTimeout(saveTimer);
    saveTimer = setTimeout(save, SAVE_DELAY_MS);
  }

  // Saves anything still waiting in the debounce, right now.
  function flush() {
    if (saveTimer === null) return saving;
    clearTimeout(saveTimer);
    saveTimer = null;
    return save();
  }

  function renderPresets(presets) {
    presetList.replaceChildren();
    if (presets.length === 0) {
      const empty = document.createElement("li");
      empty.textContent = "No presets";
      presetList.append(empty);
      return;
    }
    for (const preset of presets) {
      const item = document.createElement("li");
      const name = document.createElement("span");
      name.textContent = preset.name;
      const target = document.createElement("span");
      target.className = "target";
      target.textContent = preset.target === "*" ? "Same format" : preset.target;
      item.append(name, target);
      presetList.append(item);
    }
  }

  function pickFolder() {
    const options = { directory: true, multiple: false };
    if (customFolder.value) options.defaultPath = customFolder.value;
    // The plugin's global API when it is there; the raw command otherwise.
    if (tauri.dialog && tauri.dialog.open) return tauri.dialog.open(options);
    return invoke("plugin:dialog|open", { options: options });
  }

  function joinPath(dir, name) {
    const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
    return dir.endsWith(sep) ? dir + name : dir + sep + name;
  }

  function reveal(path) {
    if (tauri.opener && tauri.opener.revealItemInDir) return tauri.opener.revealItemInDir(path);
    return invoke("plugin:opener|reveal_item_in_dir", { paths: [path] });
  }

  form.addEventListener("input", scheduleSave);
  form.addEventListener("change", scheduleSave);
  form.addEventListener("submit", function (e) {
    e.preventDefault();
  });

  chooseFolder.addEventListener("click", async function () {
    try {
      const picked = await pickFolder();
      // Cancelled dialogs answer null; an array only shows up if someone
      // flips `multiple` on, but belt and braces.
      const folder = Array.isArray(picked) ? picked[0] : picked;
      if (!folder) return;
      customFolder.value = folder;
      scheduleSave();
    } catch (err) {
      showStatus("Could not open the folder picker: " + err, true);
    }
  });

  openFolder.addEventListener("click", async function () {
    if (!configDir) return;
    try {
      // Revealing settings.json opens its folder in the file manager, and
      // the default opener permissions allow exactly that. Writing the last
      // good settings first makes sure there is a settings.json to reveal,
      // even on a fresh install where nobody has changed a thing yet.
      await flush();
      await invoke("settings_set", { settings: settings });
      await reveal(joinPath(configDir, "settings.json"));
    } catch (err) {
      showStatus("Could not open the folder: " + err, true);
    }
  });

  window.addEventListener("beforeunload", function () {
    flush();
  });

  async function load() {
    try {
      const [got, presets] = await Promise.all([invoke("settings_get"), invoke("presets_list")]);
      configDir = got.configDir || null;
      settings = Object.assign({}, got);
      delete settings.configDir;
      render();
      renderPresets(presets);
    } catch (err) {
      showStatus("Could not load the settings: " + err, true);
    }
  }

  load();
})();
