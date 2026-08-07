package main

import (
	"net/http"
	"strings"
)

type uiMode string

const (
	modeFull   uiMode = "full"
	modeLegacy uiMode = "legacy"
	modeAuto   uiMode = "auto"
)

const modeCookie = "ui"

// No bare "PPC" marker: it also matches PowerPC Macintosh user agents.
var legacyUAMarkers = []string{
	"windows ce",
	"windows mobile",
	"iemobile",
	"msie 4.",
	"msie 5.",
}

func parseMode(raw string) (uiMode, bool) {
	switch uiMode(strings.ToLower(strings.TrimSpace(raw))) {
	case modeFull:
		return modeFull, true
	case modeLegacy:
		return modeLegacy, true
	case modeAuto:
		return modeAuto, true
	}
	return modeFull, false
}

func classifyUserAgent(ua string) uiMode {
	lower := strings.ToLower(ua)
	for _, marker := range legacyUAMarkers {
		if strings.Contains(lower, marker) {
			return modeLegacy
		}
	}
	return modeFull
}

func resolveMode(r *http.Request) uiMode {
	if raw := r.URL.Query().Get(modeCookie); raw != "" {
		if mode, ok := parseMode(raw); ok {
			if mode == modeAuto {
				return classifyUserAgent(r.UserAgent())
			}
			return mode
		}
	}
	if c, err := r.Cookie(modeCookie); err == nil {
		if mode, ok := parseMode(c.Value); ok && mode != modeAuto {
			return mode
		}
	}
	return classifyUserAgent(r.UserAgent())
}

func applyMode(w http.ResponseWriter, r *http.Request) uiMode {
	raw := r.URL.Query().Get(modeCookie)
	if raw == "" {
		return resolveMode(r)
	}
	mode, ok := parseMode(raw)
	if !ok {
		return resolveMode(r)
	}
	if mode == modeAuto {
		http.SetCookie(w, &http.Cookie{Name: modeCookie, Value: "", Path: "/", MaxAge: -1})
		return classifyUserAgent(r.UserAgent())
	}
	http.SetCookie(w, &http.Cookie{Name: modeCookie, Value: string(mode), Path: "/", MaxAge: 30 * 24 * 3600})
	return mode
}

func uiModeLabel(mode uiMode) string {
	if mode == modeLegacy {
		return "Legacy HTML"
	}
	return "Full"
}

// A re-run link would only land on the mode already offered.
func modeSwitcherHTML(current uiMode) string {
	other := modeLegacy
	if current == modeLegacy {
		other = modeFull
	}
	return `<table class="modes" cellspacing="0" cellpadding="0"><tr><td class="hint">Mode: <b>` +
		uiModeLabel(current) + `</b> (<a href="/?ui=` + string(other) + `">switch to ` +
		uiModeLabel(other) + `</a>)</td></tr></table>`
}
