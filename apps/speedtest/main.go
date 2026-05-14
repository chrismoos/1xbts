package main

import (
	"bytes"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha1"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"html/template"
	"io"
	"log"
	"net/http"
	"os"
	"strconv"
	"strings"
	"time"
)

const (
	defaultAddr           = ":5656"
	defaultSize           = 64 * 1024
	defaultNoScriptDLSize = 128 * 1024
	maxSize               = 512 * 1024
)

var sizes = []int{
	4 * 1024,
	8 * 1024,
	16 * 1024,
	32 * 1024,
	64 * 1024,
	128 * 1024,
	256 * 1024,
	512 * 1024,
}

type server struct {
	downloadPayload []byte
	uploadPayload   []byte
	signingKey      []byte
}

type pageData struct {
	Sizes                 []sizeOption
	DefaultSize           int
	DefaultNoScriptDLSize int
}

type sizeOption struct {
	Bytes int
	Label string
}

type resultData struct {
	Title     string
	Direction string
	Mode      string
	Bytes     int64
	Seconds   string
	Kbps      string
	Error     string
}

func main() {
	s := &server{
		downloadPayload: makeGIFPayload(maxSize),
		uploadPayload:   makeTextPayload(maxSize),
		signingKey:      makeSigningKey(),
	}
	mux := http.NewServeMux()
	mux.HandleFunc("/", s.index)
	mux.HandleFunc("/warmup.gif", s.warmupGIF)
	mux.HandleFunc("/download.bin", s.downloadBin)
	mux.HandleFunc("/download-html", s.downloadStart)
	mux.HandleFunc("/download-start", s.downloadStart)
	mux.HandleFunc("/download-result", s.downloadResult)
	mux.HandleFunc("/upload-form", s.uploadForm)
	mux.HandleFunc("/upload", s.upload)
	mux.HandleFunc("/healthz", healthz)

	addr := os.Getenv("PORT")
	if addr == "" {
		addr = defaultAddr
	} else if !strings.HasPrefix(addr, ":") {
		addr = ":" + addr
	}

	log.Printf("speed-test listening on %s", addr)
	log.Fatal(http.ListenAndServe(addr, mux))
}

func (s *server) index(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	setHTMLHeaders(w)
	if err := indexTemplate.Execute(w, pageData{Sizes: sizeOptions(), DefaultSize: defaultSize, DefaultNoScriptDLSize: defaultNoScriptDLSize}); err != nil {
		log.Printf("render index: %v", err)
	}
}

func (s *server) downloadBin(w http.ResponseWriter, r *http.Request) {
	n, err := requestSize(r, "bytes")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	setNoStore(w)
	w.Header().Set("Content-Type", "image/gif")
	w.Header().Set("Content-Length", strconv.Itoa(n))
	_, _ = w.Write(s.downloadPayload[:n])
}

func (s *server) warmupGIF(w http.ResponseWriter, _ *http.Request) {
	payload := makeGIFPayload(43)
	setNoStore(w)
	w.Header().Set("Content-Type", "image/gif")
	w.Header().Set("Content-Length", strconv.Itoa(len(payload)))
	_, _ = w.Write(payload)
}

func (s *server) downloadStart(w http.ResponseWriter, r *http.Request) {
	n, err := requestSize(r, "bytes")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	setHTMLHeaders(w)
	w.Header().Set("X-Accel-Buffering", "no")

	flusher, _ := w.(http.Flusher)
	startNS := time.Now().UnixNano()
	resultPath := fmt.Sprintf("/download-result?bytes=%d&start=%d&sig=%s", n, startNS, s.signDownload(n, startNS))

	fmt.Fprint(w, legacyHeader("Download Test"))
	fmt.Fprintf(w, "<p>Receiving %s. Please wait for the result below.</p>\n", formatBytes(int64(n)))
	fmt.Fprint(w, "<p class=\"status\">Receiving test data...</p>\n<!--")
	if flusher != nil {
		flusher.Flush()
	}

	remaining := n
	chunk := bytes.Repeat([]byte("x"), 256)
	for remaining > 0 {
		writeLen := len(chunk)
		if remaining < writeLen {
			writeLen = remaining
		}
		if _, err := w.Write(chunk[:writeLen]); err != nil {
			return
		}
		remaining -= writeLen
		if flusher != nil {
			flusher.Flush()
		}
	}
	escapedPath := template.HTMLEscapeString(resultPath)
	fmt.Fprintf(w, "--><iframe class=\"resultFrame\" src=\"%s\"><a href=\"%s\">Show Result</a></iframe>", escapedPath, escapedPath)
	fmt.Fprint(w, legacyFooter())
}

func (s *server) downloadResult(w http.ResponseWriter, r *http.Request) {
	n, err := requestSize(r, "bytes")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	startNS, err := strconv.ParseInt(r.URL.Query().Get("start"), 10, 64)
	if err != nil || startNS <= 0 {
		http.Error(w, "invalid timestamp", http.StatusBadRequest)
		return
	}
	if !s.verifyDownload(n, startNS, r.URL.Query().Get("sig")) {
		http.Error(w, "invalid signature", http.StatusBadRequest)
		return
	}
	start := time.Unix(0, startNS)
	now := time.Now()
	if start.After(now.Add(5*time.Second)) || now.Sub(start) > 10*time.Minute {
		http.Error(w, "expired test", http.StatusBadRequest)
		return
	}

	setHTMLHeaders(w)
	elapsed := now.Sub(start)
	fmt.Fprint(w, resultFrameHeader())
	_ = resultTemplate.Execute(w, resultData{
		Title:     "Download Result",
		Direction: "Download",
		Mode:      "browser callback",
		Bytes:     int64(n),
		Seconds:   formatSeconds(elapsed),
		Kbps:      formatKbps(int64(n), elapsed),
	})
	fmt.Fprint(w, "</body></html>")
}

func (s *server) uploadForm(w http.ResponseWriter, r *http.Request) {
	n, err := requestSize(r, "bytes")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	setHTMLHeaders(w)
	data := struct {
		Bytes   int
		Label   string
		Payload string
		Token   string
	}{
		Bytes:   n,
		Label:   formatBytes(int64(n)),
		Payload: string(s.uploadPayload[:n]),
		Token:   nonce(),
	}
	if err := uploadFormTemplate.Execute(w, data); err != nil {
		log.Printf("render upload form: %v", err)
	}
}

func (s *server) upload(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST required", http.StatusMethodNotAllowed)
		return
	}
	setHTMLHeaders(w)
	r.Body = http.MaxBytesReader(w, r.Body, int64(maxSize+4096))
	start := time.Now()
	n, err := io.Copy(io.Discard, r.Body)
	elapsed := time.Since(start)
	if err != nil {
		renderUploadResult(w, r, resultData{
			Title:     "Upload Error",
			Direction: "Upload",
			Mode:      modeLabel(r),
			Error:     "Upload was too large or could not be read.",
		})
		return
	}

	renderUploadResult(w, r, resultData{
		Title:     "Upload Result",
		Direction: "Upload",
		Mode:      modeLabel(r),
		Bytes:     uploadPayloadBytes(r, n),
		Seconds:   formatSeconds(elapsed),
		Kbps:      formatKbps(uploadPayloadBytes(r, n), elapsed),
	})
}

func renderUploadResult(w http.ResponseWriter, r *http.Request, data resultData) {
	if r.URL.Query().Get("js") == "1" {
		token := jsString(r.URL.Query().Get("token"))
		if data.Error != "" {
			fmt.Fprintf(w, "<!doctype html><title>Done</title><script>parent.speedTestUploadDone('%s',0,0,'%s');</script>", token, jsString(data.Error))
			return
		}
		fmt.Fprintf(w, "<!doctype html><title>Done</title><script>parent.speedTestUploadDone('%s',%d,%q,'');</script>", token, data.Bytes, data.Seconds)
		return
	}

	fmt.Fprint(w, legacyHeader(data.Title))
	_ = resultTemplate.Execute(w, data)
	fmt.Fprint(w, legacyFooter())
}

func healthz(w http.ResponseWriter, _ *http.Request) {
	w.Header().Set("Content-Type", "text/plain; charset=us-ascii")
	fmt.Fprint(w, "ok\n")
}

func requestSize(r *http.Request, name string) (int, error) {
	raw := r.URL.Query().Get(name)
	if raw == "" {
		return defaultSize, nil
	}
	n, err := strconv.Atoi(raw)
	if err != nil || n <= 0 {
		return 0, fmt.Errorf("invalid size")
	}
	for _, allowed := range sizes {
		if n == allowed {
			return n, nil
		}
	}
	return 0, fmt.Errorf("unsupported size")
}

func makeTextPayload(n int) []byte {
	alphabet := []byte("0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz")
	out := make([]byte, n)
	if _, err := rand.Read(out); err != nil {
		for i := range out {
			out[i] = alphabet[i%len(alphabet)]
		}
		return out
	}
	for i := range out {
		out[i] = alphabet[int(out[i])%len(alphabet)]
	}
	return out
}

func makeSigningKey() []byte {
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		sum := sha256.Sum256([]byte(fmt.Sprintf("%d", time.Now().UnixNano())))
		return sum[:]
	}
	return key
}

func (s *server) signDownload(bytes int, startNS int64) string {
	mac := hmac.New(sha256.New, s.signingKey)
	fmt.Fprintf(mac, "download:%d:%d", bytes, startNS)
	return hex.EncodeToString(mac.Sum(nil))
}

func (s *server) verifyDownload(bytes int, startNS int64, sig string) bool {
	expected := s.signDownload(bytes, startNS)
	got, err := hex.DecodeString(sig)
	if err != nil {
		return false
	}
	want, err := hex.DecodeString(expected)
	if err != nil {
		return false
	}
	return hmac.Equal(got, want)
}

func makeGIFPayload(n int) []byte {
	base := []byte{
		'G', 'I', 'F', '8', '9', 'a',
		1, 0, 1, 0,
		0x80, 0, 0,
		0, 0, 0,
		255, 255, 255,
		',',
		0, 0, 0, 0, 1, 0, 1, 0,
		0,
		2, 2, 0x44, 1, 0,
		';',
	}
	if n < len(base) {
		return append([]byte(nil), base[:n]...)
	}
	padding := n - len(base)
	out := make([]byte, 0, n)
	out = append(out, base[:len(base)-1]...)
	if padding >= 3 {
		out = append(out, 0x21, 0xfe)
		room := padding - 3
		for room > 0 {
			consume := room
			if consume > 256 {
				consume = 256
			}
			if room-consume == 1 {
				consume--
			}
			blockLen := consume - 1
			out = append(out, byte(blockLen))
			for i := 0; i < blockLen; i++ {
				out = append(out, 'x')
			}
			room -= consume
		}
		out = append(out, 0)
		padding = 0
	}
	for padding > 0 {
		out = append(out, 'x')
		padding--
	}
	out = append(out, ';')
	return out
}

func sizeOptions() []sizeOption {
	out := make([]sizeOption, 0, len(sizes))
	for _, n := range sizes {
		out = append(out, sizeOption{Bytes: n, Label: formatBytes(int64(n))})
	}
	return out
}

func setHTMLHeaders(w http.ResponseWriter) {
	setNoStore(w)
	w.Header().Set("Content-Type", "text/html; charset=us-ascii")
	w.Header().Set("Content-Language", "en")
}

func setNoStore(w http.ResponseWriter) {
	w.Header().Set("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
	w.Header().Set("Pragma", "no-cache")
	w.Header().Set("Expires", "0")
}

func formatBytes(n int64) string {
	if n%1024 == 0 {
		return fmt.Sprintf("%d KiB", n/1024)
	}
	return fmt.Sprintf("%d bytes", n)
}

func formatSeconds(d time.Duration) string {
	if d <= 0 {
		return "0.001"
	}
	return fmt.Sprintf("%.3f", d.Seconds())
}

func formatKbps(bytes int64, d time.Duration) string {
	if bytes <= 0 || d <= 0 {
		return "0.0"
	}
	return fmt.Sprintf("%.1f", float64(bytes*8)/d.Seconds()/1000)
}

func nonce() string {
	sum := sha1.Sum([]byte(fmt.Sprintf("%d", time.Now().UnixNano())))
	return hex.EncodeToString(sum[:])[:12]
}

func modeLabel(r *http.Request) string {
	if r.URL.Query().Get("js") == "1" {
		return "browser observed"
	}
	return "server observed"
}

func uploadPayloadBytes(r *http.Request, fallback int64) int64 {
	n, err := requestSize(r, "bytes")
	if err != nil {
		return fallback
	}
	return int64(n)
}

func jsString(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `'`, `\'`)
	s = strings.ReplaceAll(s, "\n", `\n`)
	s = strings.ReplaceAll(s, "\r", "")
	return s
}

func legacyHeader(title string) string {
	return `<!doctype html>
<html lang="en">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=us-ascii">
<meta http-equiv="Content-Language" content="en">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>` + template.HTMLEscapeString(title) + `</title>
<style>` + pageCSS + `</style>
</head>
<body><div class="shell"><table class="box" cellspacing="0" cellpadding="0">` + brandBarHTML + `<tr><td class="body">`
}

func legacyFooter() string {
	return `<p class="nav"><a href="/">Return</a></p></td></tr></table></div></body></html>`
}

func resultFrameHeader() string {
	return `<!doctype html>
<html lang="en">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=us-ascii">
<meta http-equiv="Content-Language" content="en">
<style>` + pageCSS + `
body{background:#dcdcdc;font-size:10px}
.result{font-size:10px}
.result th,.result td{padding:1px}
</style>
</head>
<body class="frameBody">`
}

const brandBarHTML = `<tr><td class="bar"><span class="brandWrap"><span class="brandIcon"><svg viewBox="0 0 104 104" width="24" height="24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"><defs><linearGradient id="b" x1="0%" y1="0%" x2="100%" y2="100%"><stop offset="0%" stop-color="#6366f1"/><stop offset="100%" stop-color="#10b981"/></linearGradient><linearGradient id="t" x1="0%" y1="0%" x2="0%" y2="100%"><stop offset="0%" stop-color="#818cf8"/><stop offset="100%" stop-color="#34d399"/></linearGradient></defs><rect width="104" height="104" rx="16" fill="#0c0e14"/><path d="M14 36A38 38 0 0136 14M68 14A38 38 0 0190 36" fill="none" stroke="url(#b)" stroke-width="5" stroke-linecap="round" opacity=".35"/><path d="M24 40A22 22 0 0140 24M64 24A22 22 0 0180 40" fill="none" stroke="url(#b)" stroke-width="5" stroke-linecap="round" opacity=".6"/><path d="M34 42A12 12 0 0142 34M62 34A12 12 0 0170 42" fill="none" stroke="url(#b)" stroke-width="5" stroke-linecap="round" opacity=".9"/><circle cx="52" cy="32" r="4" fill="#818cf8"/><line x1="52" y1="36" x2="52" y2="58" stroke="url(#t)" stroke-width="5" stroke-linecap="round"/><line x1="52" y1="58" x2="38" y2="88" stroke="url(#t)" stroke-width="4" stroke-linecap="round"/><line x1="52" y1="58" x2="66" y2="88" stroke="url(#t)" stroke-width="4" stroke-linecap="round"/><line x1="43" y1="68" x2="61" y2="68" stroke="#34d399" stroke-width="3" stroke-linecap="round" opacity=".6"/><line x1="40" y1="78" x2="64" y2="78" stroke="#34d399" stroke-width="3" stroke-linecap="round" opacity=".6"/></svg></span><span class="brandText">1xBTS</span><span class="brandSub">Speed Test</span></span></td></tr>`

var indexTemplate = template.Must(template.New("index").Parse(`<!doctype html>
<html lang="en">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=us-ascii">
<meta http-equiv="Content-Language" content="en">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>1xBTS Speed Test</title>
<style>` + pageCSS + `</style>
</head>
<body>
<div class="shell">
<table class="box" cellspacing="0" cellpadding="0">
` + brandBarHTML + `
<tr><td class="body">
<p class="tag">Link Tester</p>
<form name="speedForm" action="/" method="get">
<table class="form" cellspacing="0" cellpadding="0">
<tr>
<td class="lbl"><label for="bytes">Size</label></td>
<td>
<select id="bytes" name="bytes">
{{range .Sizes}}<option value="{{.Bytes}}"{{if eq .Bytes $.DefaultSize}} selected{{end}}>{{.Label}}</option>{{end}}
</select>
</td>
</tr>
<tr><td colspan="2" class="hint">Run one direction at a time.</td></tr>
</table>
</form>

<div id="jsbox" class="panel">
<p class="mode">JavaScript mode</p>
<p>
<input id="downBtn" class="btn" type="button" value="Download" onclick="speedTestDownload()">
<input id="upBtn" class="btn" type="button" value="Upload" onclick="speedTestUpload()">
</p>
<div id="status" class="status">Ready.</div>
<table class="result" cellspacing="0" cellpadding="0">
<tr><th>Dir</th><th>Mode</th><th>Bytes</th><th>Sec</th><th>kbps</th></tr>
<tr><td id="rdir">-</td><td id="rmode">-</td><td id="rbytes">-</td><td id="rsec">-</td><td id="rkbps">-</td></tr>
</table>
</div>

<noscript>
<div class="panel">
<p class="mode">No JavaScript mode</p>
<p class="hint">These tests use plain links and forms. Download uses a browser callback.</p>
<form action="/download-start" method="get">
<p><input type="hidden" name="bytes" value="{{.DefaultNoScriptDLSize}}"><input class="btn" type="submit" value="Download"></p>
</form>
<form action="/upload-form" method="get">
<p><input type="hidden" name="bytes" value="{{.DefaultSize}}"><input class="btn" type="submit" value="Upload"></p>
</form>
<p class="nav">
Download sizes:
{{range .Sizes}}<a href="/download-start?bytes={{.Bytes}}">{{.Label}}</a> {{end}}
</p>
<p class="nav">
Upload sizes:
{{range .Sizes}}<a href="/upload-form?bytes={{.Bytes}}">{{.Label}}</a> {{end}}
</p>
</div>
</noscript>

<form id="uploadForm" action="/upload" method="post" target="uploadFrame" style="display:none">
<textarea name="payload" id="uploadPayload"></textarea>
</form>
<iframe name="uploadFrame" id="uploadFrame" title="upload target" style="display:none"></iframe>

<p class="foot">Application-layer throughput. Disable proxies/compression for best results.</p>
</td></tr>
</table>
</div>
<script type="text/javascript">
var speedTestBusy = false;
var speedTestStart = 0;
var speedTestToken = "";
var speedTestTimeout = null;

function speedTestSize() {
  var sel = document.getElementById("bytes");
  return parseInt(sel.options[sel.selectedIndex].value, 10);
}

function speedTestSetBusy(busy) {
  speedTestBusy = busy;
  document.getElementById("downBtn").disabled = busy;
  document.getElementById("upBtn").disabled = busy;
}

function speedTestStatus(text) {
  document.getElementById("status").innerHTML = text;
}

function speedTestResult(dir, mode, bytes, seconds) {
  var kbps = 0;
  if (seconds > 0) {
    kbps = (bytes * 8 / seconds / 1000);
  }
  document.getElementById("rdir").innerHTML = dir;
  document.getElementById("rmode").innerHTML = mode;
  document.getElementById("rbytes").innerHTML = bytes;
  document.getElementById("rsec").innerHTML = seconds.toFixed ? seconds.toFixed(3) : seconds;
  document.getElementById("rkbps").innerHTML = kbps.toFixed ? kbps.toFixed(1) : kbps;
}

function speedTestNonce() {
  return "" + (new Date()).getTime() + "" + Math.floor(Math.random() * 100000);
}

function speedTestDownload() {
  if (speedTestBusy) { return; }
  var bytes = speedTestSize();
  var warmup = new Image();
  speedTestSetBusy(true);
  speedTestStatus("Preparing...");
  speedTestTimeout = window.setTimeout(function() {
    speedTestSetBusy(false);
    speedTestStatus("Timed out.");
  }, 90000);
  warmup.onload = function() {
    speedTestDownloadMeasured(bytes);
  };
  warmup.onerror = function() {
    window.clearTimeout(speedTestTimeout);
    speedTestStatus("Preparing failed.");
    speedTestSetBusy(false);
  };
  warmup.src = "/warmup.gif?nonce=" + speedTestNonce();
}

function speedTestDownloadMeasured(bytes) {
  var img = new Image();
  var start = new Date();
  speedTestStatus("Downloading...");
  img.onload = function() {
    window.clearTimeout(speedTestTimeout);
    var seconds = ((new Date()).getTime() - start.getTime()) / 1000;
    speedTestResult("Down", "browser", bytes, seconds);
    speedTestStatus("Done.");
    speedTestSetBusy(false);
  };
  img.onerror = function() {
    window.clearTimeout(speedTestTimeout);
    speedTestStatus("Download failed.");
    speedTestSetBusy(false);
  };
  img.src = "/download.bin?bytes=" + bytes + "&nonce=" + speedTestNonce();
}

function speedTestPayload(bytes) {
  var block = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  var out = "";
  while (out.length < bytes) {
    out += block;
  }
  return out.substring(0, bytes);
}

function speedTestUpload() {
  if (speedTestBusy) { return; }
  var bytes = speedTestSize();
  speedTestToken = speedTestNonce();
  document.getElementById("uploadPayload").value = speedTestPayload(bytes);
  document.getElementById("uploadForm").action = "/upload?js=1&bytes=" + bytes + "&token=" + speedTestToken;
  speedTestStart = (new Date()).getTime();
  speedTestSetBusy(true);
  speedTestStatus("Uploading...");
  speedTestTimeout = window.setTimeout(function() {
    speedTestSetBusy(false);
    speedTestStatus("Timed out.");
  }, 90000);
  document.getElementById("uploadForm").submit();
}

function speedTestUploadDone(token, bytes, secondsText, err) {
  if (token != speedTestToken) { return; }
  window.clearTimeout(speedTestTimeout);
  if (err) {
    speedTestStatus(err);
    speedTestSetBusy(false);
    return;
  }
  var elapsed = ((new Date()).getTime() - speedTestStart) / 1000;
  speedTestResult("Up", "browser", bytes, elapsed);
  speedTestStatus("Done.");
  speedTestSetBusy(false);
}
</script>
</body>
</html>`))

var uploadFormTemplate = template.Must(template.New("uploadForm").Parse(`<!doctype html>
<html lang="en">
<head>
<meta http-equiv="Content-Type" content="text/html; charset=us-ascii">
<meta http-equiv="Content-Language" content="en">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>1xBTS Upload Test</title>
<style>` + pageCSS + `</style>
</head>
<body>
<div class="shell">
<table class="box" cellspacing="0" cellpadding="0">
` + brandBarHTML + `
<tr><td class="body">
<p class="tag">Upload {{.Label}}</p>
<p class="hint">Press Start and wait for the result page.</p>
<form action="/upload?bytes={{.Bytes}}" method="post">
<input type="hidden" name="token" value="{{.Token}}">
<textarea name="payload" class="hiddenPayload">{{.Payload}}</textarea>
<p><input class="btn" type="submit" value="Start Upload"></p>
</form>
<p class="nav"><a href="/">Return</a></p>
</td></tr>
</table>
</div>
</body>
</html>`))

var resultTemplate = template.Must(template.New("result").Parse(`
{{if .Error}}
<p class="err">{{.Error}}</p>
{{else}}
<table class="result" cellspacing="0" cellpadding="0">
<tr><th>Dir</th><th>Mode</th><th>Bytes</th><th>Sec</th><th>kbps</th></tr>
<tr><td>{{.Direction}}</td><td>{{.Mode}}</td><td>{{.Bytes}}</td><td>{{.Seconds}}</td><td>{{.Kbps}}</td></tr>
</table>
{{end}}
`))

const pageCSS = `
html,body{margin:0;padding:0}
body{background:#0c0e14;color:#e2e8f0;font:12px Verdana,Arial,Helvetica,sans-serif}
a{color:#818cf8;text-decoration:underline}
.shell{width:auto;max-width:520px;margin:8px;display:block}
.box{width:100%;background:#111827;border-top:2px solid #475569;border-left:2px solid #475569;border-right:2px solid #020617;border-bottom:2px solid #020617}
.bar{background:#0c0e14;color:#f1f5f9;font-weight:bold;padding:5px 6px;border-bottom:1px solid #6366f1}
.brandWrap{display:block;white-space:nowrap}
.brandIcon{display:inline-block;vertical-align:middle;width:24px;height:24px;margin-right:6px}
.brandText{display:inline-block;vertical-align:middle;color:#34d399;font-size:16px;letter-spacing:1px;text-shadow:1px 1px #020617}
.brandSub{display:inline-block;vertical-align:middle;color:#818cf8;font-size:11px;margin-left:6px;font-weight:normal}
.body{padding:8px;background:#111827}
.tag{margin:0 0 6px 0;font-weight:bold;color:#f1f5f9}
.panel{background:#1f2937;border-top:1px solid #475569;border-left:1px solid #475569;border-right:1px solid #020617;border-bottom:1px solid #020617;margin:7px 0;padding:6px}
.mode{margin:0 0 5px 0;font-weight:bold;color:#34d399}
.form{width:100%;margin-bottom:5px}
.lbl{width:42px;font-weight:bold}
select,input{font:12px Arial,Helvetica,sans-serif}
.btn{background:#111827;color:#f1f5f9;border-top:2px solid #64748b;border-left:2px solid #64748b;border-right:2px solid #020617;border-bottom:2px solid #020617;padding:2px 7px;margin:2px}
.btnDisabled{color:#64748b}
select{background:#0c0e14;color:#f1f5f9;border:1px solid #475569}
.hint,.foot{font-size:11px;color:#94a3b8}
.status{background:#020617;color:#34d399;border-top:1px solid #020617;border-left:1px solid #020617;border-right:1px solid #475569;border-bottom:1px solid #475569;padding:4px;margin:4px 0;min-height:14px}
.resultFrame{width:100%;height:82px;border:0;margin:4px 0}
.result{width:100%;background:#020617;color:#e2e8f0;border:1px solid #475569;font-size:10px;table-layout:fixed}
.result th{background:#111827;color:#34d399;text-align:left;padding:2px;border-bottom:1px solid #6366f1}
.result td{border-top:1px solid #1f2937;padding:2px;word-wrap:break-word}
.result th,.result td{line-height:1.1}
.nav{font-size:11px;line-height:1.5}
.err{background:#020617;border:1px solid #f87171;padding:4px;color:#f87171}
.hiddenPayload{width:1px;height:1px;position:absolute;left:-999px;top:-999px}
@media (min-width:540px){.shell{margin:12px auto}}
@media (max-width:260px){.shell{margin:0}.body{padding:5px}.result{font-size:10px}.btn{padding:2px 4px}}
`
