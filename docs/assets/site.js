/* nlvm site — NL syntax highlighting, terminal animation, scroll reveal */
(function () {
  "use strict";

  /* ---------- NL syntax highlighter ---------- */

  var KEYWORDS = new Set([
    "namespace", "use", "class", "interface", "enum",
    "public", "private", "protected", "static", "readonly",
    "extends", "implements", "construct", "destruct",
    "if", "else", "while", "for", "break", "continue", "return", "match", "default",
    "try", "catch", "finally", "throw", "throws",
    "new", "this", "super", "instanceof", "ref",
    "auto", "void", "int", "float", "bool", "string",
    "null", "true", "false"
  ]);

  function escapeHtml(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // Tokenizes raw NL source and returns highlighted HTML.
  var NL_TOKEN = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/)|("(?:[^"\\]|\\.)*")|\b(\d+(?:\.\d+)?)\b|\b([A-Za-z_][A-Za-z0-9_]*)\b|(=>|\?\?|\?:|[|])/g;

  function highlightNl(src) {
    var html = "";
    var last = 0;
    var m;
    NL_TOKEN.lastIndex = 0;
    while ((m = NL_TOKEN.exec(src)) !== null) {
      html += escapeHtml(src.slice(last, m.index));
      last = NL_TOKEN.lastIndex;
      var text = escapeHtml(m[0]);
      if (m[1]) {
        html += '<span class="tok-com">' + text + "</span>";
      } else if (m[2]) {
        html += '<span class="tok-str">' + text + "</span>";
      } else if (m[3]) {
        html += '<span class="tok-num">' + text + "</span>";
      } else if (m[4]) {
        if (KEYWORDS.has(m[4])) {
          html += '<span class="tok-kw">' + text + "</span>";
        } else if (/^[A-Z]/.test(m[4])) {
          html += '<span class="tok-type">' + text + "</span>";
        } else {
          html += text;
        }
      } else if (m[5]) {
        html += '<span class="tok-punct">' + text + "</span>";
      }
    }
    html += escapeHtml(src.slice(last));
    return html;
  }

  // Shell blocks: color "$ " prompts and comment lines; leave output dim.
  function highlightSh(src) {
    return src.split("\n").map(function (line) {
      if (/^\$\s/.test(line)) {
        return '<span class="prompt">$</span> ' + escapeHtml(line.slice(2));
      }
      if (/^#/.test(line)) {
        return '<span class="tok-com">' + escapeHtml(line) + "</span>";
      }
      return '<span class="out">' + escapeHtml(line) + "</span>";
    }).join("\n");
  }

  document.querySelectorAll("pre > code").forEach(function (code) {
    var pre = code.parentElement;
    var raw = code.textContent.replace(/^\n+|\s+$/g, "");
    if (pre.classList.contains("nl")) {
      code.innerHTML = highlightNl(raw);
    } else if (pre.classList.contains("sh")) {
      code.innerHTML = highlightSh(raw);
    } else {
      code.textContent = raw;
    }

    var btn = document.createElement("button");
    btn.className = "copy-btn";
    btn.type = "button";
    btn.textContent = "copy";
    btn.addEventListener("click", function () {
      var text = pre.classList.contains("sh")
        ? raw.split("\n").filter(function (l) { return /^\$\s/.test(l); })
             .map(function (l) { return l.slice(2); }).join("\n") || raw
        : raw;
      navigator.clipboard.writeText(text).then(function () {
        btn.textContent = "copied";
        setTimeout(function () { btn.textContent = "copy"; }, 1400);
      });
    });
    pre.appendChild(btn);
  });

  /* ---------- scroll reveal ---------- */

  var revealed = document.querySelectorAll(".reveal");
  if ("IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) {
          e.target.classList.add("visible");
          io.unobserve(e.target);
        }
      });
    }, { threshold: 0.12 });
    revealed.forEach(function (el) { io.observe(el); });
  } else {
    revealed.forEach(function (el) { el.classList.add("visible"); });
  }

  /* ---------- animated terminal ---------- */

  var term = document.getElementById("terminal-demo");
  if (!term) return;

  // A step is either a typed command or printed output lines.
  var SCRIPT = [
    { type: "cmd", text: "nlc --version" },
    { type: "out", lines: [{ cls: "out", text: "nlc 0.5.0 (nlvm-specs 0.8.44)" }] },
    { type: "cmd", text: "nlc -o out/ Main.nl" },
    { type: "cmd", text: "nlvm out/hello.Main.nlp" },
    { type: "out", lines: [{ cls: "ok", text: "Hello, world!" }] }
  ];

  var reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var cursor = document.createElement("span");
  cursor.className = "cursor";

  function renderAllInstant() {
    var html = "";
    SCRIPT.forEach(function (step) {
      if (step.type === "cmd") {
        html += '<span class="prompt">$</span> ' + escapeHtml(step.text) + "\n";
      } else {
        step.lines.forEach(function (l) {
          html += '<span class="' + l.cls + '">' + escapeHtml(l.text) + "</span>\n";
        });
      }
    });
    term.innerHTML = html;
  }

  if (reduced) {
    renderAllInstant();
    return;
  }

  var started = false;
  function startTerminal() {
    if (started) return;
    started = true;
    term.innerHTML = "";
    term.appendChild(cursor);
    runStep(0);
  }

  function runStep(i) {
    if (i >= SCRIPT.length) return;
    var step = SCRIPT[i];
    if (step.type === "cmd") {
      var prompt = document.createElement("span");
      prompt.className = "prompt";
      prompt.textContent = "$ ";
      term.insertBefore(prompt, cursor);
      typeText(step.text, 0, function () {
        term.insertBefore(document.createTextNode("\n"), cursor);
        setTimeout(function () { runStep(i + 1); }, 250);
      });
    } else {
      step.lines.forEach(function (l) {
        var span = document.createElement("span");
        span.className = l.cls;
        span.textContent = l.text;
        term.insertBefore(span, cursor);
        term.insertBefore(document.createTextNode("\n"), cursor);
      });
      setTimeout(function () { runStep(i + 1); }, 500);
    }
  }

  function typeText(text, pos, done) {
    if (pos >= text.length) { done(); return; }
    term.insertBefore(document.createTextNode(text[pos]), cursor);
    setTimeout(function () { typeText(text, pos + 1, done); }, 28 + Math.random() * 40);
  }

  if ("IntersectionObserver" in window) {
    var tio = new IntersectionObserver(function (entries) {
      if (entries.some(function (e) { return e.isIntersecting; })) {
        tio.disconnect();
        setTimeout(startTerminal, 350);
      }
    }, { threshold: 0.4 });
    tio.observe(term);
  } else {
    startTerminal();
  }
})();
