/**
 * The one story every sample file tells: Harbor Coffee, a small roastery in Portland, Maine,
 * launching a monthly coffee subscription. Every number a file shows is worked out here once, so
 * the plan page, the workbook, the PDF and the deck agree.
 */

/** The app's calm palette, and the few tints the pictures need around it. */
export const COLOUR = {
  ink: "#141412",
  paper: "#f6f3ec",
  green: "#1f6f4a",
  greenDark: "#17563a",
  greenLight: "#2f8a5f",
  sand: "#e6dfd0",
  stone: "#cfc7b6",
  roast: "#5b3a22",
} as const;

/** The brand's typefaces, as Chrome and Quick Look find them on a Mac. */
export const TYPE = {
  display: "Optima",
  text: "Avenir Next",
  letter: "Georgia",
} as const;

export const BUSINESS = {
  name: "Harbor Coffee",
  fullName: "Harbor Coffee Roasters",
  owner: "Maya Ellis",
  street: "48 Commercial Street",
  city: "Portland",
  region: "ME",
  postcode: "04101",
  phone: "+1 207 555 0148",
  email: "hello@harborcoffee.example",
  web: "https://harborcoffee.example",
  subscription: "Harbor Monthly",
} as const;

/** The day every file is dated: the Saturday before the launch photos. */
export const STORY_DATE = new Date("2026-03-14T14:30:00Z");

export interface Coffee {
  sku: string;
  name: string;
  origin: string;
  process: string;
  roast: string;
  grams: number;
  dollars: number;
  notes: string;
}

export const COFFEES: readonly Coffee[] = [
  {
    sku: "HC-101",
    name: "Morning Tide",
    origin: "Huila, Colombia",
    process: "Washed",
    roast: "Medium",
    grams: 340,
    dollars: 19,
    notes: "Cocoa, orange, brown sugar",
  },
  {
    sku: "HC-102",
    name: "Lighthouse",
    origin: "Guji, Ethiopia",
    process: "Natural",
    roast: "Light",
    grams: 340,
    dollars: 22,
    notes: "Blueberry, jasmine, honey",
  },
  {
    sku: "HC-103",
    name: "Breakwater",
    origin: "Cerrado, Brazil",
    process: "Pulped natural",
    roast: "Medium-dark",
    grams: 340,
    dollars: 17,
    notes: "Hazelnut, caramel, dark chocolate",
  },
  {
    sku: "HC-104",
    name: "Fog Line Decaf",
    origin: "Cajamarca, Peru",
    process: "Swiss Water",
    roast: "Medium",
    grams: 340,
    dollars: 20,
    notes: "Milk chocolate, almond",
  },
  {
    sku: "HC-105",
    name: "Low Tide Espresso",
    origin: "Brazil and Guatemala",
    process: "Blend",
    roast: "Medium-dark",
    grams: 340,
    dollars: 18,
    notes: "Toffee, cherry, cocoa",
  },
  {
    sku: "HC-106",
    name: "Harbor Light",
    origin: "Nyeri, Kenya",
    process: "Washed",
    roast: "Light",
    grams: 340,
    dollars: 24,
    notes: "Blackcurrant, grapefruit, cane sugar",
  },
];

export const [MORNING_TIDE, LIGHTHOUSE, BREAKWATER] = COFFEES as [Coffee, Coffee, Coffee];

/** What a bag costs to fill and a box to send, in whole dollars. */
export const COST = { perBagDollars: 7, shippingDollars: 5 } as const;

export interface Plan {
  name: string;
  bags: number;
  dollars: number;
  /** The share of subscribers the model expects on this plan, out of 100. */
  sharePercent: number;
  for: string;
}

export const PLANS: readonly Plan[] = [
  { name: "Weekday", bags: 1, dollars: 18, sharePercent: 50, for: "One person, one cup a day" },
  { name: "Household", bags: 2, dollars: 34, sharePercent: 38, for: "Two people, or a big habit" },
  { name: "Office", bags: 4, dollars: 64, sharePercent: 12, for: "A small team" },
];

export const planCostDollars = (plan: Plan) =>
  plan.bags * COST.perBagDollars + COST.shippingDollars;

export const planMarginPercent = (plan: Plan) =>
  Math.round(((plan.dollars - planCostDollars(plan)) / plan.dollars) * 100);

export interface ForecastMonth {
  month: string;
  subscribers: number;
  revenueDollars: number;
  costDollars: number;
  profitDollars: number;
}

const FORECAST_MONTHS = [
  ["Apr 2026", 60],
  ["May 2026", 95],
  ["Jun 2026", 130],
  ["Jul 2026", 160],
  ["Aug 2026", 185],
  ["Sep 2026", 215],
  ["Oct 2026", 250],
  ["Nov 2026", 290],
  ["Dec 2026", 340],
  ["Jan 2027", 365],
  ["Feb 2027", 390],
  ["Mar 2027", 420],
] as const;

/** Each month of the first year: subscribers spread over the plans by their share. */
export const FORECAST: readonly ForecastMonth[] = FORECAST_MONTHS.map(([month, subscribers]) => {
  const perPlan = PLANS.map((plan) => ({ plan, people: (subscribers * plan.sharePercent) / 100 }));
  const revenueDollars = Math.round(
    perPlan.reduce((sum, { plan, people }) => sum + people * plan.dollars, 0),
  );
  const costDollars = Math.round(
    perPlan.reduce((sum, { plan, people }) => sum + people * planCostDollars(plan), 0),
  );
  return {
    month,
    subscribers,
    revenueDollars,
    costDollars,
    profitDollars: revenueDollars - costDollars,
  };
});

export const LAST_MONTH = FORECAST[FORECAST.length - 1] as ForecastMonth;

export const yearMarginPercent = () => {
  const revenue = FORECAST.reduce((sum, month) => sum + month.revenueDollars, 0);
  const profit = FORECAST.reduce((sum, month) => sum + month.profitDollars, 0);
  return Math.round((profit / revenue) * 100);
};

export const LAUNCH_STEPS = [
  { date: "Mar 18", step: "Photograph all three bags" },
  { date: "Mar 21", step: "Subscribe page goes live for pre-orders" },
  { date: "Mar 24", step: "Email the 1,200 people on the shop list" },
  { date: "Apr 1", step: "First boxes ship" },
] as const;

export interface Customer {
  name: string;
  city: string;
  plan: string;
  since: string;
  grind: string;
  favourite: string;
}

export const CUSTOMERS: readonly Customer[] = [
  ["Ava Thompson", "Portland, ME", "Household", "2026-03-02", "Whole bean", "Morning Tide"],
  ["Ben Carter", "South Portland, ME", "Weekday", "2026-03-02", "Pour-over", "Lighthouse"],
  ["Chloe Nguyen", "Boston, MA", "Weekday", "2026-03-03", "Espresso", "Low Tide Espresso"],
  ["Daniel Brooks", "Falmouth, ME", "Office", "2026-03-03", "Drip", "Breakwater"],
  ["Ella Fischer", "Brunswick, ME", "Household", "2026-03-04", "Whole bean", "Harbor Light"],
  ["Felix Moreau", "Portsmouth, NH", "Weekday", "2026-03-05", "French press", "Breakwater"],
  ["Grace Kim", "Portland, ME", "Weekday", "2026-03-05", "Pour-over", "Lighthouse"],
  ["Henry Walsh", "Camden, ME", "Household", "2026-03-06", "Drip", "Morning Tide"],
  ["Isla Romero", "Burlington, VT", "Weekday", "2026-03-07", "Whole bean", "Fog Line Decaf"],
  ["Jack Sullivan", "Providence, RI", "Office", "2026-03-08", "Espresso", "Low Tide Espresso"],
  ["Kira Patel", "Cambridge, MA", "Household", "2026-03-08", "Pour-over", "Harbor Light"],
  ["Leo Martins", "Westbrook, ME", "Weekday", "2026-03-09", "Drip", "Morning Tide"],
  ["Mia Johansson", "Bar Harbor, ME", "Weekday", "2026-03-10", "Whole bean", "Lighthouse"],
  ["Noah Bennett", "Saco, ME", "Household", "2026-03-10", "French press", "Breakwater"],
  ["Olivia Reyes", "Concord, NH", "Weekday", "2026-03-11", "Pour-over", "Harbor Light"],
  ["Owen Clarke", "Portland, ME", "Office", "2026-03-11", "Drip", "Morning Tide"],
  ["Priya Shah", "Somerville, MA", "Weekday", "2026-03-12", "Espresso", "Low Tide Espresso"],
  ["Quinn Murphy", "Kennebunk, ME", "Household", "2026-03-12", "Whole bean", "Fog Line Decaf"],
  ["Rosa Lindqvist", "Belfast, ME", "Weekday", "2026-03-13", "Pour-over", "Lighthouse"],
  ["Sam Okafor", "Portland, ME", "Household", "2026-03-13", "Drip", "Morning Tide"],
].map(([name, city, plan, since, grind, favourite]) => ({
  name: name as string,
  city: city as string,
  plan: plan as string,
  since: since as string,
  grind: grind as string,
  favourite: favourite as string,
}));

/** A small, fixed pseudo-random sequence (mulberry32), so every run draws the same scene. */
export function seededRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = state;
    mixed = Math.imul(mixed ^ (mixed >>> 15), mixed | 1);
    mixed ^= mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61);
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4_294_967_296;
  };
}

export const dollars = (amount: number) => `$${amount.toLocaleString("en-US")}`;
