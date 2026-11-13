// Renders docs/manual/manual.html to docs/Weighbridge-Manual.pdf with page numbers.
//   NODE_PATH=<a node_modules containing playwright-core> node docs/manual/build-pdf.js [chrome-path]
const { chromium } = require("playwright-core");
const path = require("path");
(async () => {
  const exe = process.argv[2] || process.env.CHROME_PATH;
  const b = await chromium.launch(exe ? { executablePath: exe } : {});
  const p = await b.newPage();
  await p.goto("file://" + path.join(__dirname, "manual.html"), { waitUntil: "load" });
  await p.pdf({
    path: path.join(__dirname, "..", "Weighbridge-Manual.pdf"),
    format: "A4",
    printBackground: true,
    preferCSSPageSize: true,
    displayHeaderFooter: true,
    headerTemplate: "<div></div>",
    footerTemplate:
      '<div style="font:8px sans-serif;color:#777;width:100%;padding:0 16mm;display:flex;justify-content:space-between"><span>Weighbridge System Manual</span><span><span class="pageNumber"></span> / <span class="totalPages"></span></span></div>',
  });
  await b.close();
  console.log("written docs/Weighbridge-Manual.pdf");
})().catch((e) => { console.error(e); process.exit(1); });
