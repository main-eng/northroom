/**
 * The words in the sample folder: the plan page, notes, tables, code, the live subscribe page,
 * the PDF's and the letter's designed HTML, a contact card and the launch calendar.
 */
import { logoMark } from "./art";
import {
  BUSINESS,
  COFFEES,
  COLOUR,
  COST,
  CUSTOMERS,
  FORECAST,
  LAST_MONTH,
  LAUNCH_STEPS,
  MORNING_TIDE,
  PLANS,
  STORY_DATE,
  TYPE,
  dollars,
  planCostDollars,
  planMarginPercent,
  seededRandom,
  yearMarginPercent,
} from "./story";

const lines = (...text: readonly string[]) => `${text.join("\n")}\n`;
const fence = "```";

export function planPage(): string {
  const planRows = PLANS.map(
    (plan) =>
      `| ${plan.name} | ${plan.bags} | ${dollars(plan.dollars)} | ${planMarginPercent(plan)}% |`,
  );
  return lines(
    "---",
    `title: ${BUSINESS.subscription} launch plan`,
    `owner: ${BUSINESS.owner}`,
    "status: In progress",
    "launch: 2026-04-01",
    "tags: [subscription, launch]",
    "---",
    "",
    `# ${BUSINESS.subscription} launch plan`,
    "",
    `${BUSINESS.subscription} sends fresh coffee to your door every month. We roast on Monday and ship the same day, so every bag arrives within a week of roasting.`,
    "",
    "## Goals for the first year",
    "",
    `- ${LAST_MONTH.subscribers} subscribers by March 2027`,
    `- Keep the year's margin near ${yearMarginPercent()}%`,
    "- Ship every box within a day of roasting",
    "",
    "## Plans",
    "",
    "| Plan | Bags a month | Price a month | Margin |",
    "| --- | ---: | ---: | ---: |",
    ...planRows,
    "",
    "The full numbers are in the [subscription model][model], and every coffee is in the [price list][prices].",
    "",
    "## Launch steps",
    "",
    ...LAUNCH_STEPS.map((step, index) => `${index + 1}. ${step.step} (${step.date})`),
    "",
    "## This week",
    "",
    "- [x] Order 500 printed boxes",
    "- [x] Approve the new label",
    "- [ ] Photograph the bags",
    "- [ ] Write the welcome card",
    "",
    "> I'd pay for this just to stop running out on a Tuesday.",
    ">",
    "> — a regular at the Saturday market",
    "",
    "## How a box is priced",
    "",
    `${fence}text`,
    "cost   = bags × $7 + $5 shipping",
    "margin = (price − cost) ÷ price",
    fence,
    "",
    "---",
    "",
    "Questions go to Maya. The roast schedule lives in `Code/roast-schedule.yaml`.",
    "",
    "[model]: ../Spreadsheets/Subscription%20model.xlsx",
    "[prices]: ../Spreadsheets/Price%20list.csv",
  );
}

export function notesText(): string {
  return lines(
    "Cupping notes, Saturday 14 March",
    "",
    "Morning Tide (Huila, washed)",
    "  Dry: cocoa powder, orange peel.",
    "  Cup: sweet and round. Brown sugar as it cools. Our best all-rounder.",
    "  Verdict: the first bag in every Weekday box.",
    "",
    "Lighthouse (Guji, natural)",
    "  Dry: blueberry jam.",
    "  Cup: bright, floral, honey finish. Light roast holds up well in milk too.",
    "",
    "Breakwater (Cerrado, pulped natural)",
    "  Dry: hazelnut.",
    "  Cup: caramel and dark chocolate. Heavy body. Good for French press.",
    "",
    "To do",
    "  - Roast 2 kg more Morning Tide for the photo shoot on Wednesday.",
    "  - Ask Sam about printing the tasting cards on heavier paper.",
  );
}

const delimited = (rows: readonly (readonly (string | number)[])[], separator: string) => {
  const cell = (value: string | number) => {
    const text = String(value);
    return separator === "," && /[",\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  };
  return rows.map((row) => row.map(cell).join(separator)).join("\n") + "\n";
};

export function priceListCsv(): string {
  return delimited(
    [
      ["SKU", "Coffee", "Origin", "Process", "Roast", "Size (g)", "Price (USD)", "Tasting notes"],
      ...COFFEES.map((coffee) => [
        coffee.sku,
        coffee.name,
        coffee.origin,
        coffee.process,
        coffee.roast,
        coffee.grams,
        coffee.dollars.toFixed(2),
        coffee.notes,
      ]),
    ],
    ",",
  );
}

export function customersTsv(): string {
  return delimited(
    [
      ["Name", "City", "Plan", "Since", "Grind", "Favourite"],
      ...CUSTOMERS.map((customer) => [
        customer.name,
        customer.city,
        customer.plan,
        customer.since,
        customer.grind,
        customer.favourite,
      ]),
    ],
    "\t",
  );
}

export const SCHEMA_SQL = lines(
  "-- Harbor Coffee orders: who subscribes, what we roast, and every box we send.",
  "",
  "CREATE TABLE customers (",
  "  id INTEGER PRIMARY KEY,",
  "  name TEXT NOT NULL,",
  "  city TEXT NOT NULL,",
  "  plan TEXT NOT NULL CHECK (plan IN ('Weekday', 'Household', 'Office')),",
  "  since TEXT NOT NULL",
  ");",
  "",
  "CREATE TABLE coffees (",
  "  sku TEXT PRIMARY KEY,",
  "  name TEXT NOT NULL,",
  "  origin TEXT NOT NULL,",
  "  price_dollars INTEGER NOT NULL",
  ");",
  "",
  "CREATE TABLE orders (",
  "  id INTEGER PRIMARY KEY,",
  "  customer_id INTEGER NOT NULL REFERENCES customers (id),",
  "  sku TEXT NOT NULL REFERENCES coffees (sku),",
  "  bags INTEGER NOT NULL,",
  "  shipped_on TEXT NOT NULL",
  ");",
  "",
  "CREATE INDEX orders_by_customer ON orders (customer_id);",
);

const quoted = (text: string) => `'${text.replace(/'/g, "''")}'`;

/** The rows the orders database holds, as SQL. */
export function ordersSql(): string {
  const random = seededRandom(53);
  const customers = CUSTOMERS.map(
    (customer, index) =>
      `INSERT INTO customers VALUES (${index + 1}, ${quoted(customer.name)}, ${quoted(customer.city)}, ${quoted(customer.plan)}, ${quoted(customer.since)});`,
  );
  const coffees = COFFEES.map(
    (coffee) =>
      `INSERT INTO coffees VALUES (${quoted(coffee.sku)}, ${quoted(coffee.name)}, ${quoted(coffee.origin)}, ${coffee.dollars});`,
  );
  const orders = Array.from({ length: 48 }, (_, index) => {
    const customer = 1 + Math.floor(random() * CUSTOMERS.length);
    const coffee = COFFEES[Math.floor(random() * COFFEES.length)]!;
    const bags = 1 + Math.floor(random() * 2);
    const day = 2 + Math.floor((index / 48) * 12);
    return `INSERT INTO orders VALUES (${index + 1}, ${customer}, ${quoted(coffee.sku)}, ${bags}, '2026-03-${String(day).padStart(2, "0")}');`;
  });
  return lines("BEGIN;", ...customers, ...coffees, ...orders, "COMMIT;", "VACUUM;");
}

/** The code folder's files, by name: small, real programs a roastery might keep. */
export function codeFiles(): Record<string, string> {
  return {
    "roast_profile.py": lines(
      '"""Rate of rise and development time for one roast, from the probe log."""',
      "",
      "from dataclasses import dataclass",
      "",
      "",
      "@dataclass",
      "class Reading:",
      "    seconds: int",
      "    bean_temp_c: float",
      "",
      "",
      "FIRST_CRACK_C = 196.0",
      "",
      "",
      "def rate_of_rise(readings: list[Reading]) -> list[float]:",
      '    """Degrees per minute between each reading and the next."""',
      "    return [",
      "        (b.bean_temp_c - a.bean_temp_c) / (b.seconds - a.seconds) * 60",
      "        for a, b in zip(readings, readings[1:])",
      "    ]",
      "",
      "",
      "def development_ratio(readings: list[Reading]) -> float:",
      '    """The share of the roast after first crack."""',
      "    crack = next(r.seconds for r in readings if r.bean_temp_c >= FIRST_CRACK_C)",
      "    total = readings[-1].seconds",
      "    return (total - crack) / total",
      "",
      "",
      'if __name__ == "__main__":',
      "    log = [Reading(0, 92), Reading(120, 141), Reading(240, 168), Reading(360, 186),",
      "           Reading(480, 197), Reading(560, 204), Reading(600, 208)]",
      '    print("RoR:", [round(r, 1) for r in rate_of_rise(log)])',
      '    print(f"Development: {development_ratio(log):.0%}")',
    ),
    "inventory.rs": lines(
      "//! How many roasts the green coffee on the shelf still makes.",
      "",
      "struct GreenCoffee {",
      "    name: &'static str,",
      "    kg_on_hand: f32,",
      "}",
      "",
      "/// Green coffee one batch in the 6 kg roaster takes.",
      "const KG_PER_ROAST: f32 = 5.0;",
      "",
      "impl GreenCoffee {",
      "    fn roasts_left(&self) -> u32 {",
      "        (self.kg_on_hand / KG_PER_ROAST).floor() as u32",
      "    }",
      "}",
      "",
      "fn main() {",
      "    let shelf = [",
      '        GreenCoffee { name: "Huila, Colombia", kg_on_hand: 138.0 },',
      '        GreenCoffee { name: "Guji, Ethiopia", kg_on_hand: 54.5 },',
      '        GreenCoffee { name: "Cerrado, Brazil", kg_on_hand: 21.0 },',
      "    ];",
      "    for coffee in &shelf {",
      "        let left = coffee.roasts_left();",
      '        let warning = if left < 6 { "  <- reorder" } else { "" };',
      '        println!("{:<18} {:>3} roasts{}", coffee.name, left, warning);',
      "    }",
      "}",
    ),
    "subscription.ts": lines(
      "/** The monthly plans and what each box costs us to fill and send. */",
      "export interface Plan {",
      "  name: string;",
      "  bags: number;",
      "  priceDollars: number;",
      "}",
      "",
      `const COST_PER_BAG_DOLLARS = ${COST.perBagDollars};`,
      `const SHIPPING_DOLLARS = ${COST.shippingDollars};`,
      "",
      "export const PLANS: Plan[] = [",
      ...PLANS.map(
        (plan) => `  { name: "${plan.name}", bags: ${plan.bags}, priceDollars: ${plan.dollars} },`,
      ),
      "];",
      "",
      "export function marginPercent(plan: Plan): number {",
      "  const cost = plan.bags * COST_PER_BAG_DOLLARS + SHIPPING_DOLLARS;",
      "  return Math.round(((plan.priceDollars - cost) / plan.priceDollars) * 100);",
      "}",
      "",
      "for (const plan of PLANS) {",
      "  console.log(`${plan.name}: $${plan.priceDollars} a month, ${marginPercent(plan)}% margin`);",
      "}",
    ),
    "shipping.js": lines(
      "// When a box roasted on Monday reaches each zone, counting business days only.",
      "",
      "const DAYS_IN_TRANSIT = { local: 1, northeast: 2, everywhere: 3 };",
      "",
      "export function arrivalDate(roastedOn, zone) {",
      "  const date = new Date(roastedOn);",
      "  let days = DAYS_IN_TRANSIT[zone];",
      "  while (days > 0) {",
      "    date.setDate(date.getDate() + 1);",
      "    const weekday = date.getDay();",
      "    if (weekday !== 0 && weekday !== 6) days -= 1;",
      "  }",
      "  return date.toISOString().slice(0, 10);",
      "}",
      "",
      'console.log(arrivalDate("2026-03-30", "everywhere")); // 2026-04-02, a Thursday',
    ),
    "coffees.json": `${JSON.stringify(
      {
        roaster: BUSINESS.fullName,
        updated: "2026-03-14",
        coffees: COFFEES.map((coffee) => ({
          sku: coffee.sku,
          name: coffee.name,
          origin: coffee.origin,
          process: coffee.process,
          roast: coffee.roast,
          grams: coffee.grams,
          priceDollars: coffee.dollars,
          notes: coffee.notes.split(", ").map((note) => note.toLowerCase()),
        })),
      },
      null,
      2,
    )}\n`,
    "roast-schedule.yaml": lines(
      "# Which coffee goes in the roaster when. Batches are 5 kg of green coffee.",
      "week_of: 2026-03-16",
      "roaster: Diedrich IR-5",
      "days:",
      "  monday:",
      "    - coffee: Morning Tide",
      "      batches: 4",
      "      note: Subscription boxes ship the same afternoon",
      "    - coffee: Lighthouse",
      "      batches: 2",
      "  wednesday:",
      "    - coffee: Breakwater",
      "      batches: 2",
      "    - coffee: Low Tide Espresso",
      "      batches: 3",
      "  friday:",
      "    - coffee: Fog Line Decaf",
      "      batches: 1",
      "      note: Clean the drum first",
    ),
    "roaster.toml": lines(
      "# Settings for the shop's roaster and its probe logger.",
      "",
      "[roaster]",
      'model = "Diedrich IR-5"',
      "batch_kg = 5.0",
      "charge_temp_c = 205",
      "",
      "[logger]",
      'port = "/dev/tty.usbserial-1420"',
      "sample_every_s = 2",
      'save_to = "~/Roasts"',
      "",
      "[alarms]",
      "first_crack_c = 196",
      "drop_after_crack_s = [75, 120]",
    ),
    "backup-orders.sh": lines(
      "#!/bin/sh",
      "# Copies the orders database to the backup drive, one file per day, keeping 30 days.",
      "set -eu",
      "",
      'source="$HOME/Harbor Coffee/Spreadsheets/Orders.sqlite"',
      'backups="/Volumes/Backup/orders"',
      "",
      'mkdir -p "$backups"',
      'sqlite3 "$source" ".backup \'$backups/orders-$(date +%F).sqlite\'"',
      'find "$backups" -name "orders-*.sqlite" -mtime +30 -delete',
      'echo "Backed up to $backups"',
    ),
    "brand.css": lines(
      "/* Harbor Coffee's colours and type, shared by the website and the subscribe page. */",
      ":root {",
      `  --harbor-green: ${COLOUR.green};`,
      `  --ink: ${COLOUR.ink};`,
      `  --paper: ${COLOUR.paper};`,
      `  --sand: ${COLOUR.sand};`,
      `  --roast: ${COLOUR.roast};`,
      `  --display: "${TYPE.display}", "Candara", serif;`,
      `  --text: "${TYPE.text}", "Segoe UI", sans-serif;`,
      "}",
      "",
      "body {",
      "  background: var(--paper);",
      "  color: var(--ink);",
      "  font: 17px/1.55 var(--text);",
      "}",
      "",
      "h1,",
      "h2 {",
      "  font-family: var(--display);",
      "  letter-spacing: -0.01em;",
      "}",
      "",
      ".button {",
      "  background: var(--harbor-green);",
      "  color: var(--paper);",
      "  border-radius: 999px;",
      "  padding: 12px 24px;",
      "}",
    ),
    "schema.sql": SCHEMA_SQL,
  };
}

/** The live subscribe page: pick a plan and a grind, and see the price and the first ship date. */
export function subscribePage(): string {
  const plans = PLANS.map(
    (plan, index) => `<label class="plan">
        <input type="radio" name="plan" value="${plan.dollars}" data-bags="${plan.bags}" data-name="${plan.name}"${index === 1 ? " checked" : ""}>
        <span class="card"><strong>${plan.name}</strong><span class="price">${dollars(plan.dollars)}<small>/month</small></span><span class="for">${plan.bags} ${plan.bags === 1 ? "bag" : "bags"} · ${plan.for}</span></span>
      </label>`,
  ).join("\n      ");
  const coffees = COFFEES.map((coffee) => `<option>${coffee.name}</option>`).join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Subscribe · ${BUSINESS.subscription}</title>
<style>
  :root { --green: ${COLOUR.green}; --ink: ${COLOUR.ink}; --paper: ${COLOUR.paper}; --sand: ${COLOUR.sand}; }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--paper); color: var(--ink); font: 17px/1.5 "${TYPE.text}", system-ui, sans-serif; }
  header { display: flex; align-items: center; gap: 12px; padding: 20px 32px; border-bottom: 1px solid #0001; }
  header svg { width: 36px; height: 36px; }
  header b { font-family: "${TYPE.display}", serif; letter-spacing: 0.24em; font-size: 18px; }
  main { max-width: 760px; margin: 0 auto; padding: 48px 24px 64px; }
  h1 { font-family: "${TYPE.display}", serif; font-size: 48px; line-height: 1.05; margin: 0 0 12px; }
  .lede { margin: 0 0 32px; opacity: 0.75; }
  .plans { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; }
  .plan input { position: absolute; opacity: 0; }
  .card { display: flex; flex-direction: column; gap: 4px; padding: 18px; border-radius: 14px; background: #fff8; border: 2px solid transparent; cursor: pointer; transition: border-color 0.15s, transform 0.15s; }
  .card:hover { transform: translateY(-2px); }
  .plan input:checked + .card { border-color: var(--green); background: #fff; }
  .price { font-family: "${TYPE.display}", serif; font-size: 30px; font-weight: 600; }
  .price small { font-size: 14px; opacity: 0.6; font-family: "${TYPE.text}", sans-serif; }
  .for { font-size: 14px; opacity: 0.7; }
  .row { display: flex; gap: 12px; margin: 24px 0; }
  .row label { flex: 1; font-size: 14px; font-weight: 600; }
  select { display: block; width: 100%; margin-top: 6px; padding: 10px 12px; font: inherit; border-radius: 10px; border: 1px solid #0002; background: #fff; }
  .total { display: flex; justify-content: space-between; align-items: center; padding: 20px 24px; border-radius: 14px; background: var(--green); color: var(--paper); }
  .total strong { font-family: "${TYPE.display}", serif; font-size: 34px; }
  button { font: inherit; font-weight: 600; border: 0; border-radius: 999px; padding: 12px 24px; background: var(--paper); color: var(--green); cursor: pointer; }
  .done { margin-top: 16px; color: var(--green); font-weight: 600; min-height: 1.5em; }
</style>
</head>
<body>
  <header><svg viewBox="0 0 100 100">${logoMark(COLOUR.green, 5)}</svg><b>HARBOR</b></header>
  <main>
    <h1>Fresh coffee,<br>on your schedule.</h1>
    <p class="lede">Roasted on Monday, at your door by Thursday. Pause or stop any time.</p>
    <div class="plans">
      ${plans}
    </div>
    <div class="row">
      <label>Coffee<select id="coffee"><option>Roaster's choice</option>${coffees}</select></label>
      <label>Grind<select id="grind"><option>Whole bean</option><option>Pour-over</option><option>Drip</option><option>French press</option><option>Espresso</option></select></label>
    </div>
    <div class="total">
      <div><div id="summary"></div><strong id="price"></strong></div>
      <button id="start">Start subscription</button>
    </div>
    <p class="done" id="done"></p>
  </main>
<script>
  const summary = document.getElementById("summary");
  const price = document.getElementById("price");
  function nextMonday(from) {
    const date = new Date(from);
    date.setDate(date.getDate() + ((8 - date.getDay()) % 7 || 7));
    return date.toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
  }
  function update() {
    const plan = document.querySelector("input[name=plan]:checked");
    const grind = document.getElementById("grind").value.toLowerCase();
    const coffee = document.getElementById("coffee").value;
    summary.textContent = plan.dataset.name + " · " + plan.dataset.bags + " × " + coffee + ", " + grind;
    price.textContent = "$" + plan.value + " a month";
  }
  document.querySelectorAll("input, select").forEach((input) => input.addEventListener("change", update));
  document.getElementById("start").addEventListener("click", () => {
    document.getElementById("done").textContent = "You're in. Your first box is roasted " + nextMonday(Date.now()) + ".";
  });
  update();
</script>
</body>
</html>
`;
}

const pageStyle = `
  @page { size: Letter; margin: 0; }
  * { box-sizing: border-box; }
  html, body { margin: 0; padding: 0; -webkit-print-color-adjust: exact; print-color-adjust: exact; }
  body { font-family: "${TYPE.text}", sans-serif; color: ${COLOUR.ink}; }
  .page { position: relative; width: 8.5in; height: 11in; overflow: hidden; background: ${COLOUR.paper}; break-after: page; }
  .page:last-child { break-after: auto; }
  .pad { padding: 64px 72px; }
  .kicker { font-size: 11px; font-weight: 600; letter-spacing: 0.24em; color: ${COLOUR.green}; text-transform: uppercase; }
  h1, h2 { font-family: "${TYPE.display}", serif; font-weight: 600; margin: 0; }
  h1 { font-size: 64px; line-height: 1; letter-spacing: -0.01em; }
  h2 { font-size: 38px; line-height: 1.1; margin: 10px 0 18px; }
  p { font-size: 14px; line-height: 1.6; margin: 0 0 12px; }
  .foot { position: absolute; left: 72px; right: 72px; bottom: 40px; display: flex; justify-content: space-between; font-size: 10px; letter-spacing: 0.12em; text-transform: uppercase; opacity: 0.55; }
  table { width: 100%; border-collapse: collapse; font-size: 12px; }
  th { text-align: left; font-size: 10px; letter-spacing: 0.14em; text-transform: uppercase; color: ${COLOUR.green}; padding: 8px 6px; border-bottom: 1.5px solid ${COLOUR.ink}; }
  td { padding: 9px 6px; border-bottom: 1px solid #0000001a; }
  td.number, th.number { text-align: right; font-variant-numeric: tabular-nums; }
`;

/** The launch plan as a designed, three-page PDF: the cover, the plans, the first year. */
export function proposalHtml(pictures: { bags: string; box: string }): string {
  const most = Math.max(...FORECAST.map((month) => month.subscribers));
  const bars = FORECAST.map((month, index) => {
    const height = (month.subscribers / most) * 190;
    const x = 18 + index * 49;
    return `<rect x="${x}" y="${220 - height}" width="32" height="${height}" rx="3" fill="${index === FORECAST.length - 1 ? COLOUR.green : "#1f6f4a66"}"/>
<text x="${x + 16}" y="${212 - height}" text-anchor="middle" font-size="10" fill="${COLOUR.ink}">${month.subscribers}</text>
<text x="${x + 16}" y="240" text-anchor="middle" font-size="9" fill="${COLOUR.ink}" opacity="0.6">${month.month.slice(0, 3)}</text>`;
  }).join("");
  const cards = PLANS.map(
    (plan, index) => `<div class="card${index === 1 ? " chosen" : ""}">
  <div class="kicker">${plan.name}</div>
  <div class="price">${dollars(plan.dollars)}<span>/month</span></div>
  <p>${plan.bags} ${plan.bags === 1 ? "bag" : "bags"} of 340 g<br>${plan.for}</p>
  <div class="margin">${planMarginPercent(plan)}% margin · costs ${dollars(planCostDollars(plan))}</div>
</div>`,
  ).join("");
  const coffeeRows = COFFEES.map(
    (coffee) =>
      `<tr><td><b>${coffee.name}</b></td><td>${coffee.origin}</td><td>${coffee.process}</td><td>${coffee.notes}</td><td class="number">${dollars(coffee.dollars)}</td></tr>`,
  ).join("");
  const totalRevenue = FORECAST.reduce((sum, month) => sum + month.revenueDollars, 0);
  const steps = LAUNCH_STEPS.map(
    (step) => `<div class="step"><div class="date">${step.date}</div><div>${step.step}</div></div>`,
  ).join("");
  return `<!doctype html><html><head><meta charset="utf-8"><title>${BUSINESS.subscription} launch plan</title><style>${pageStyle}
  .cover-picture { height: 6.1in; background: url(${pictures.bags}) center / cover; }
  .cover .pad { padding-top: 48px; }
  .cover h1 { margin: 14px 0 18px; }
  .cover p { font-size: 16px; max-width: 5.6in; opacity: 0.8; }
  .cards { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; margin: 26px 0 34px; }
  .card { padding: 20px; border-radius: 12px; background: #fff; border: 1px solid #0000000f; }
  .card.chosen { background: ${COLOUR.green}; color: ${COLOUR.paper}; }
  .card.chosen .kicker { color: ${COLOUR.paper}; }
  .price { font-family: "${TYPE.display}", serif; font-size: 36px; font-weight: 600; margin: 8px 0; }
  .price span { font-size: 12px; font-family: "${TYPE.text}", sans-serif; opacity: 0.6; }
  .card p { font-size: 12px; }
  .margin { font-size: 10px; letter-spacing: 0.06em; opacity: 0.65; }
  .box-picture { height: 2.6in; border-radius: 12px; background: url(${pictures.box}) center / cover; margin-bottom: 30px; }
  .stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; margin: 24px 0 30px; }
  .stat { padding: 18px 20px; border-radius: 12px; background: #fff; }
  .stat b { display: block; font-family: "${TYPE.display}", serif; font-size: 34px; color: ${COLOUR.green}; }
  .stat span { font-size: 11px; opacity: 0.65; }
  .chart { background: #fff; border-radius: 12px; padding: 18px 16px 6px; }
  .steps { display: grid; grid-template-columns: repeat(4, 1fr); gap: 14px; margin-top: 28px; }
  .step { border-top: 2px solid ${COLOUR.green}; padding-top: 10px; font-size: 12px; line-height: 1.45; }
  .date { font-weight: 600; color: ${COLOUR.green}; margin-bottom: 4px; }
</style></head><body>
<section class="page cover">
  <div class="cover-picture"></div>
  <div class="pad">
    <div class="kicker">Launch plan · Spring 2026</div>
    <h1>${BUSINESS.subscription}</h1>
    <p>Fresh coffee on a schedule: roasted on Monday, at the door by Thursday. Three simple plans, six coffees in rotation, and a goal of ${LAST_MONTH.subscribers} subscribers in the first year.</p>
  </div>
  <div class="foot"><span>${BUSINESS.fullName}</span><span>${BUSINESS.city}, Maine</span></div>
</section>
<section class="page"><div class="pad">
  <div class="kicker">The offer</div>
  <h2>Three plans, one promise.</h2>
  <p>Every box is roasted to order and shipped the same day. Subscribers choose a coffee or let us pick, and can pause or stop at any time.</p>
  <div class="cards">${cards}</div>
  <div class="box-picture"></div>
  <div class="kicker">In rotation</div>
  <table><thead><tr><th>Coffee</th><th>Origin</th><th>Process</th><th>Tastes of</th><th class="number">Bag</th></tr></thead><tbody>${coffeeRows}</tbody></table>
</div><div class="foot"><span>${BUSINESS.subscription} launch plan</span><span>2</span></div></section>
<section class="page"><div class="pad">
  <div class="kicker">The first year</div>
  <h2>Slow, steady growth.</h2>
  <p>We expect most people to start on Weekday and move up to Household. The forecast keeps half of subscribers on Weekday, 38% on Household and 12% on Office.</p>
  <div class="stats">
    <div class="stat"><b>${LAST_MONTH.subscribers}</b><span>subscribers by March 2027</span></div>
    <div class="stat"><b>${dollars(totalRevenue)}</b><span>revenue in the first year</span></div>
    <div class="stat"><b>${yearMarginPercent()}%</b><span>margin after bags and shipping</span></div>
  </div>
  <div class="chart"><div class="kicker" style="margin:0 0 6px 4px">Subscribers by month</div>
    <svg viewBox="0 0 610 250" width="100%" font-family="${TYPE.text}">${bars}</svg>
  </div>
  <div class="steps">${steps}</div>
</div><div class="foot"><span>${BUSINESS.subscription} launch plan</span><span>3</span></div></section>
</body></html>`;
}

/** The wholesale letter to a café, as HTML that textutil turns into Word, RTF and OpenDocument. */
export function letterHtml(logoFile: string): string {
  // One paragraph per coffee rather than a table: textutil's tables come apart in RTF and OpenDocument.
  const prices = COFFEES.slice(0, 4)
    .map(
      (coffee) =>
        `<p class="price"><b>${coffee.name}</b> · ${coffee.origin} · $${Math.round(coffee.dollars * 2.4)} a kilo</p>`,
    )
    .join("");
  return `<!doctype html><html><head><meta charset="utf-8"><title>Wholesale coffee for Tidewater Café</title>
<style>
  body { font-family: ${TYPE.letter}, serif; font-size: 12pt; color: ${COLOUR.ink}; line-height: 1.45; }
  .brand { font-family: ${TYPE.display}, sans-serif; font-size: 22pt; letter-spacing: 6pt; color: ${COLOUR.green}; font-weight: bold; margin: 0; }
  .address { font-family: "${TYPE.text}", sans-serif; font-size: 9pt; color: #555555; margin: 0 0 28pt 0; }
  h1 { font-family: ${TYPE.display}, sans-serif; font-size: 18pt; color: ${COLOUR.ink}; margin: 18pt 0 8pt 0; }
  .price { margin: 0 0 4pt 18pt; }
  .sign { margin-top: 24pt; }
</style></head><body>
<p><img src="${logoFile}" width="64" height="64" alt="Harbor Coffee"></p>
<p class="brand">HARBOR COFFEE</p>
<p class="address">${BUSINESS.street} · ${BUSINESS.city}, ${BUSINESS.region} ${BUSINESS.postcode} · ${BUSINESS.phone} · ${BUSINESS.email}</p>
<p>${STORY_DATE.toLocaleDateString("en-US", { month: "long", day: "numeric", year: "numeric", timeZone: "UTC" })}</p>
<p>Jess Laurent<br>Tidewater Café<br>12 Fore Street<br>Portland, ME 04101</p>
<h1>Wholesale coffee for Tidewater Café</h1>
<p>Dear Jess,</p>
<p>Thank you for having us in on Thursday. As promised, here is what we can offer Tidewater: coffee roasted every Monday and delivered by bike the same afternoon, with a free grinder tune-up each spring.</p>
${prices}
<p>For your house espresso we suggest <b>Low Tide Espresso</b>; for filter, <b>${MORNING_TIDE.name}</b> (${MORNING_TIDE.notes.toLowerCase()}). Orders of 10 kg or more a week take 8% off every price above.</p>
<p>If this suits you, reply to this letter or call me and we can start with your order for the week of April 6.</p>
<p class="sign">Warm regards,<br><br><b>${BUSINESS.owner}</b><br>Founder, ${BUSINESS.fullName}</p>
</body></html>`;
}

/** The roastery's contact card, with the badge as its photo. */
export function contactCard(photoPng: Buffer): string {
  const photo = photoPng.toString("base64");
  // A vCard line is at most 75 characters; longer ones continue on lines that start with a space.
  const folded = (line: string) => {
    const parts = [line.slice(0, 75)];
    for (let at = 75; at < line.length; at += 74) parts.push(` ${line.slice(at, at + 74)}`);
    return parts.join("\r\n");
  };
  return `${[
    "BEGIN:VCARD",
    "VERSION:3.0",
    `N:Ellis;Maya;;;`,
    `FN:${BUSINESS.owner}`,
    `ORG:${BUSINESS.fullName}`,
    "TITLE:Founder and head roaster",
    `TEL;TYPE=WORK,VOICE:${BUSINESS.phone}`,
    `EMAIL;TYPE=WORK,INTERNET:${BUSINESS.email}`,
    `ADR;TYPE=WORK:;;${BUSINESS.street};${BUSINESS.city};${BUSINESS.region};${BUSINESS.postcode};United States`,
    `URL:${BUSINESS.web}`,
    "NOTE:Roasting Mondays. Wholesale and subscriptions.",
    folded(`PHOTO;ENCODING=b;TYPE=PNG:${photo}`),
    "REV:2026-03-14T14:30:00Z",
    "END:VCARD",
  ].join("\r\n")}\r\n`;
}

/** Launch day in the calendar, the other steps in its notes. Quick Look draws one event best. */
export function launchCalendar(): string {
  const steps = LAUNCH_STEPS.map((step) => `${step.date}: ${step.step}`).join("\\n");
  return `${[
    "BEGIN:VCALENDAR",
    "VERSION:2.0",
    "PRODID:-//Harbor Coffee//Launch//EN",
    "CALSCALE:GREGORIAN",
    "BEGIN:VEVENT",
    "UID:first-boxes@harborcoffee.example",
    "DTSTAMP:20260314T143000Z",
    "DTSTART:20260401T130000Z",
    "DTEND:20260401T160000Z",
    `SUMMARY:First ${BUSINESS.subscription} boxes ship`,
    "LOCATION:Roastery\\, 48 Commercial Street\\, Portland",
    `DESCRIPTION:${steps.replace(/,/g, "\\,")}`,
    "BEGIN:VALARM",
    "ACTION:DISPLAY",
    "DESCRIPTION:Boxes ship today",
    "TRIGGER:-PT1H",
    "END:VALARM",
    "END:VEVENT",
    "END:VCALENDAR",
  ].join("\r\n")}\r\n`;
}
