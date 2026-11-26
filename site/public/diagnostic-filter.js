// The Diagnostic index's filter box (/reference/diagnostics/). The page is
// generated from the compiler's registry (`render_markdown` in
// bynk-syntax/src/diagnostics.rs), which loads this script. It adds a search
// field above the first category and, as you type, shows only the codes whose
// name or summary contains every word typed, hiding a category with none left.
// A status line says how many match, and screen readers hear it. Without
// JavaScript the page is the full list, as before.

const content = document.querySelector(".sl-markdown-content");
const firstHeading = content?.querySelector(".sl-heading-wrapper.level-h2, h2");

if (content && firstHeading) {
  // Each category: its heading (or Starlight's wrapper round it) and its list.
  const groups = [...content.querySelectorAll(":scope > ul")].map((list) => {
    const heading = list.previousElementSibling;
    const items = [...list.children].map((li) => ({ li, text: li.textContent.toLowerCase() }));
    return { heading, list, items };
  });
  const total = groups.reduce((n, g) => n + g.items.length, 0);

  const box = document.createElement("div");
  box.className = "diagnostic-filter not-content";
  box.innerHTML = `
    <label for="diagnostic-filter-input">Filter the codes</label>
    <input id="diagnostic-filter-input" type="search" autocomplete="off" spellcheck="false"
      placeholder="A code or a word, such as refine or secret" aria-describedby="diagnostic-filter-status" />
    <p id="diagnostic-filter-status" role="status"></p>`;
  firstHeading.before(box);

  const input = box.querySelector("input");
  const status = box.querySelector("p");
  const codes = (n) => (n === 1 ? "1 code" : `${n} codes`);

  const apply = () => {
    const words = input.value.toLowerCase().split(/\s+/).filter(Boolean);
    let shown = 0;
    for (const g of groups) {
      let inGroup = 0;
      for (const { li, text } of g.items) {
        const match = words.every((w) => text.includes(w));
        li.hidden = !match;
        if (match) inGroup++;
      }
      g.list.hidden = inGroup === 0;
      if (g.heading) g.heading.hidden = inGroup === 0;
      shown += inGroup;
    }
    status.textContent =
      words.length === 0 ? "" : shown === 0 ? "No codes match." : `${codes(shown)} of ${total} match.`;
  };

  let timer;
  input.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(apply, 150);
  });
  // A browser can restore the field's text on back or forward navigation.
  if (input.value) apply();
}
