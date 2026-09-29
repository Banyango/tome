// tome docs theme: code-block titles, top links, "On this page" and
// previous/next cards. Loaded via `additional-js`, after mdBook's own scripts.
(function () {
  "use strict";

  const root = typeof path_to_root === "string" ? path_to_root : "";
  const main = document.querySelector("#mdbook-content main");
  if (!main) return;

  // Code blocks. A fence like ```sh title=Terminal keeps `title=Terminal` in
  // the code element's class list; shell blocks without one get "Terminal".
  const SHELLS = ["language-sh", "language-bash", "language-console", "language-shell"];
  main.querySelectorAll("pre > code").forEach((code) => {
    const m = code.className.match(/title=("([^"]*)"|\S+)/);
    let title = m ? (m[2] !== undefined ? m[2] : m[1]) : null;
    if (!title && SHELLS.some((c) => code.classList.contains(c))) title = "Terminal";
    if (!title) return;
    const pre = code.parentElement;
    const frame = document.createElement("div");
    frame.className = "code-frame";
    const label = document.createElement("div");
    label.className = "code-title";
    label.textContent = title;
    pre.parentNode.insertBefore(frame, pre);
    frame.append(label, pre);
  });

  // Links in the top bar.
  const right = document.querySelector("#mdbook-menu-bar .right-buttons");
  if (right) {
    const links = document.createElement("div");
    links.className = "top-links";
    [
      ["Docs", root + "introduction.html"],
      ["Quickstart", root + "quickstart.html"],
      ["Recipes", root + "recipes.html"],
    ].forEach(([text, href]) => {
      const a = document.createElement("a");
      a.textContent = text;
      a.href = href;
      links.append(a);
    });
    right.prepend(links);
  }

  // "On this page": the page's h2 and h3 headings, with the one in view marked.
  const headings = [...main.querySelectorAll("h2[id], h3[id]")];
  if (headings.length >= 2) {
    const nav = document.createElement("nav");
    nav.className = "page-toc";
    nav.setAttribute("aria-label", "On this page");
    nav.innerHTML = '<div class="page-toc-title">On this page</div>';
    const ul = document.createElement("ul");
    const byId = new Map();
    headings.forEach((h) => {
      const li = document.createElement("li");
      li.className = "depth-" + h.tagName[1];
      const a = document.createElement("a");
      a.href = "#" + h.id;
      a.textContent = h.textContent;
      li.append(a);
      ul.append(li);
      byId.set(h.id, a);
    });
    nav.append(ul);
    document.querySelector("#mdbook-page-wrapper").append(nav);

    const mark = () => {
      let current = headings[0];
      for (const h of headings) {
        if (h.getBoundingClientRect().top < 120) current = h;
        else break;
      }
      byId.forEach((a, id) => a.classList.toggle("active", id === current.id));
    };
    document.addEventListener("scroll", mark, { passive: true });
    mark();
  }

  // Previous/next cards: label each bottom link with its chapter's title,
  // taken from the sidebar.
  const titleOf = (href) => {
    for (const a of document.querySelectorAll("#mdbook-sidebar a[href]")) {
      if (a.href === href) {
        const clone = a.cloneNode(true);
        clone.querySelectorAll("strong").forEach((s) => s.remove());
        return clone.textContent.trim();
      }
    }
    return null;
  };
  const fillCards = () => {
    document.querySelectorAll(".nav-wrapper .mobile-nav-chapters").forEach((a) => {
      if (a.querySelector(".nav-card-title")) return;
      const title = titleOf(a.href);
      if (!title) return;
      const label = document.createElement("span");
      label.className = "nav-card-label";
      label.textContent = a.classList.contains("previous") ? "Previous" : "Next";
      const name = document.createElement("span");
      name.className = "nav-card-title";
      name.textContent = title;
      a.append(label, name);
    });
  };
  fillCards();
  window.addEventListener("load", fillCards);
})();
