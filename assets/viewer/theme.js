/**
 * viewer/theme.js — Theme switching (light / dark / system)
 *
 * Reads and writes localStorage key "noet-theme".
 * Applies theme by enabling/disabling the two <link> stylesheet elements
 * and setting data-theme on <html>.
 */

import { state } from "./state.js";

// =============================================================================
// Public API
// =============================================================================

/**
 * Initialize theme from localStorage or default to "system".
 * Must be called after DOM references are populated.
 */
export function initializeTheme() {
  const saved = localStorage.getItem("noet-theme");
  if (saved === "system" || saved === "dark" || saved === "light") {
    state.currentTheme = saved;
  } else {
    state.currentTheme = "system";
  }

  if (state.themeSelect) {
    state.themeSelect.value = state.currentTheme;
  }

  applyTheme(state.currentTheme);

  // React to OS-level preference changes when theme is "system"
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", handleSystemThemeChange);
}

/**
 * Handle <select> change events from the theme picker.
 * @param {Event} event
 */
export function handleThemeChange(event) {
  state.currentTheme = event.target.value;
  console.log(`[Noet] Theme changed to: ${state.currentTheme}`);
  applyTheme(state.currentTheme);
  localStorage.setItem("noet-theme", state.currentTheme);
}

/**
 * Apply a theme by enabling/disabling stylesheet links.
 * @param {"system"|"light"|"dark"} theme
 */
export function applyTheme(theme) {
  if (!state.themeLightLink || !state.themeDarkLink) {
    console.error("[Noet] Theme stylesheet links not found, cannot apply theme");
    return;
  }

  let effective = theme;

  if (theme === "system") {
    const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    effective = prefersDark ? "dark" : "light";
    console.log(`[Noet] System preference detected: ${effective}`);
  }

  if (effective === "dark") {
    state.themeLightLink.disabled = true;
    state.themeDarkLink.disabled = false;
    document.documentElement.setAttribute("data-theme", "dark");
  } else {
    state.themeLightLink.disabled = false;
    state.themeDarkLink.disabled = true;
    document.documentElement.setAttribute("data-theme", "light");
  }

  console.log(
    `[Noet] Theme applied: ${theme} (effective: ${effective})`,
    `| light disabled: ${state.themeLightLink.disabled}`,
    `| dark disabled: ${state.themeDarkLink.disabled}`,
  );
}

// =============================================================================
// Internal
// =============================================================================

function handleSystemThemeChange() {
  if (state.currentTheme === "system") {
    applyTheme("system");
  }
}
