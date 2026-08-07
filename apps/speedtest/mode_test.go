package main

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestClassifyUserAgent(t *testing.T) {
	cases := []struct {
		name string
		ua   string
		want uiMode
	}{
		{
			name: "windows mobile 5 pocket pc",
			ua:   "Mozilla/4.0 (compatible; MSIE 4.01; Windows CE; PPC; 240x240)",
			want: modeLegacy,
		},
		{
			name: "windows mobile smartphone",
			ua:   "Mozilla/4.0 (compatible; MSIE 4.01; Windows CE; Smartphone; 176x220)",
			want: modeLegacy,
		},
		{
			name: "iemobile",
			ua:   "Mozilla/4.0 (compatible; MSIE 6.0; Windows CE; IEMobile 7.11)",
			want: modeLegacy,
		},
		{
			name: "powerpc macintosh is not a pocket pc",
			ua:   "Mozilla/5.0 (Macintosh; U; PPC Mac OS X; en) AppleWebKit/418.9",
			want: modeFull,
		},
		{
			name: "modern browser",
			ua:   "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36",
			want: modeFull,
		},
		{
			name: "empty",
			ua:   "",
			want: modeFull,
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := classifyUserAgent(tc.ua); got != tc.want {
				t.Fatalf("classifyUserAgent(%q) = %q, want %q", tc.ua, got, tc.want)
			}
		})
	}
}

func TestResolveModeLadder(t *testing.T) {
	const legacyUA = "Mozilla/4.0 (compatible; MSIE 4.01; Windows CE; PPC; 240x240)"
	const modernUA = "Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0"

	cases := []struct {
		name   string
		target string
		cookie string
		ua     string
		want   uiMode
	}{
		{name: "parameter beats cookie", target: "/?ui=legacy", cookie: "full", ua: modernUA, want: modeLegacy},
		{name: "parameter beats user agent", target: "/?ui=full", ua: legacyUA, want: modeFull},
		{name: "cookie beats user agent", target: "/", cookie: "legacy", ua: modernUA, want: modeLegacy},
		{name: "auto parameter falls back to user agent", target: "/?ui=auto", cookie: "full", ua: legacyUA, want: modeLegacy},
		{name: "unknown parameter falls back", target: "/?ui=banana", ua: legacyUA, want: modeLegacy},
		{name: "nothing set uses user agent", target: "/", ua: legacyUA, want: modeLegacy},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodGet, tc.target, nil)
			req.Header.Set("User-Agent", tc.ua)
			if tc.cookie != "" {
				req.AddCookie(&http.Cookie{Name: modeCookie, Value: tc.cookie})
			}
			if got := resolveMode(req); got != tc.want {
				t.Fatalf("resolveMode = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestApplyModeStoresExplicitChoice(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/?ui=legacy", nil)
	rec := httptest.NewRecorder()

	if got := applyMode(rec, req); got != modeLegacy {
		t.Fatalf("applyMode = %q, want legacy", got)
	}

	cookies := rec.Result().Cookies()
	if len(cookies) != 1 || cookies[0].Name != modeCookie || cookies[0].Value != "legacy" {
		t.Fatalf("expected a stored legacy choice, got %+v", cookies)
	}
}

func TestApplyModeAutoClearsStoredChoice(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/?ui=auto", nil)
	req.Header.Set("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0")
	req.AddCookie(&http.Cookie{Name: modeCookie, Value: "legacy"})
	rec := httptest.NewRecorder()

	if got := applyMode(rec, req); got != modeFull {
		t.Fatalf("applyMode = %q, want full", got)
	}

	cookies := rec.Result().Cookies()
	if len(cookies) != 1 || cookies[0].MaxAge >= 0 {
		t.Fatalf("expected the stored choice to be cleared, got %+v", cookies)
	}
}

func TestModeSwitcherLinksToTheOtherMode(t *testing.T) {
	legacy := modeSwitcherHTML(modeLegacy)
	if !strings.Contains(legacy, `href="/?ui=full"`) {
		t.Fatalf("switcher should offer the full page: %s", legacy)
	}
	if strings.Contains(legacy, `href="/?ui=legacy"`) {
		t.Fatalf("switcher should not link to the current mode: %s", legacy)
	}

	full := modeSwitcherHTML(modeFull)
	if !strings.Contains(full, `href="/?ui=legacy"`) {
		t.Fatalf("switcher should offer the legacy page: %s", full)
	}
	if strings.Contains(full, `href="/?ui=full"`) {
		t.Fatalf("switcher should not link to the current mode: %s", full)
	}
}

func TestAutoParameterStillClearsAStoredChoice(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/?ui=auto", nil)
	req.AddCookie(&http.Cookie{Name: modeCookie, Value: "legacy"})
	rec := httptest.NewRecorder()

	applyMode(rec, req)

	cookies := rec.Result().Cookies()
	if len(cookies) != 1 || cookies[0].MaxAge >= 0 {
		t.Fatalf("expected the stored choice to be cleared, got %+v", cookies)
	}
}

func TestFullPageProbesForLegacyDOM(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.Header.Set("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0")
	rec := httptest.NewRecorder()

	s.index(rec, req)

	body := rec.Body.String()
	if !strings.Contains(body, `if (!document.getElementById || !window.Image) { location.href = "/?ui=legacy"; }`) {
		t.Fatalf("full page is missing the capability probe")
	}
	if !strings.Contains(body, `href="/?ui=legacy"`) {
		t.Fatalf("full page is missing the mode switcher")
	}
}

func TestIndexServesLegacyPageToPocketIE(t *testing.T) {
	s := testServer()
	req := httptest.NewRequest(http.MethodGet, "/", nil)
	req.Header.Set("User-Agent", "Mozilla/4.0 (compatible; MSIE 4.01; Windows CE; PPC; 240x240)")
	rec := httptest.NewRecorder()

	s.index(rec, req)

	body := rec.Body.String()
	if !strings.Contains(body, `action="/l/dl"`) {
		t.Fatalf("expected the legacy index, got %q", body)
	}
	assertPocketIESafe(t, body)
}
