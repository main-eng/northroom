/**
 * The pitch deck's slides, laid out in points on PowerPoint's 16:9 slide: the title, why a
 * subscription, the three plans, the box, the first year and the launch date.
 */
import type { Deck, DeckBox, DeckShape, HexColour } from "../../../web/scripts/office-files";
import { BUSINESS, COLOUR, FORECAST, PLANS, TYPE, dollars, planMarginPercent } from "./story";

const SLIDE = { widthPt: 960, heightPt: 540 } as const;
const MARGIN_PT = 64;

/** The deck's pictures, by their place in the deck's `pictures`. */
export const DECK_PICTURE = { bagsSquare: 0, box: 1, badge: 2 } as const;

const ink = COLOUR.ink as HexColour;
const paper = COLOUR.paper as HexColour;
const green = COLOUR.green as HexColour;
const sand = "#ebe5d8" as HexColour;

type Look = {
  sizePt: number;
  bold: boolean;
  colour: HexColour;
  display: boolean;
  align?: "left" | "center";
};

const words = (text: string, box: DeckBox, look: Look): DeckShape => ({
  kind: "words",
  text,
  ...box,
  sizePt: look.sizePt,
  bold: look.bold,
  colour: look.colour,
  typeface: look.display ? TYPE.display : TYPE.text,
  align: look.align ?? "left",
});

const fill = (box: DeckBox, colour: HexColour): DeckShape => ({ kind: "box", ...box, colour });

const title = (text: string, colour: HexColour) =>
  words(
    text,
    { xPt: MARGIN_PT, yPt: 52, widthPt: 820, heightPt: 60 },
    { sizePt: 40, bold: true, colour, display: true },
  );

const kicker = (text: string, colour: HexColour) =>
  words(
    text,
    { xPt: MARGIN_PT, yPt: 34, widthPt: 600, heightPt: 20 },
    { sizePt: 11, bold: true, colour, display: false },
  );

function titleSlide(): DeckShape[] {
  return [
    {
      kind: "picture",
      picture: DECK_PICTURE.bagsSquare,
      xPt: 420,
      yPt: 0,
      widthPt: 540,
      heightPt: 540,
    },
    words(
      "HARBOR COFFEE ROASTERS",
      { xPt: 56, yPt: 56, widthPt: 320, heightPt: 20 },
      { sizePt: 12, bold: true, colour: paper, display: false },
    ),
    words(
      BUSINESS.subscription.split(" ")[0] as string,
      { xPt: 52, yPt: 190, widthPt: 360, heightPt: 70 },
      { sizePt: 60, bold: true, colour: paper, display: true },
    ),
    words(
      BUSINESS.subscription.split(" ")[1] as string,
      { xPt: 52, yPt: 256, widthPt: 360, heightPt: 70 },
      { sizePt: 60, bold: true, colour: paper, display: true },
    ),
    words(
      "A coffee subscription · Spring 2026",
      { xPt: 56, yPt: 348, widthPt: 340, heightPt: 30 },
      { sizePt: 18, bold: false, colour: paper, display: false },
    ),
    fill({ xPt: 56, yPt: 400, widthPt: 48, heightPt: 3 }, paper),
  ];
}

function whySlide(): DeckShape[] {
  const reasons = [
    ["1,200", "people already on the shop's email list"],
    ["3 in 5", "regulars say they run out before they come back"],
    ["4 days", "from roaster to door, anywhere in New England"],
  ] as const;
  return [
    kicker("WHY A SUBSCRIPTION", green),
    title("Our regulars already buy monthly.", ink),
    ...reasons.flatMap(([number, reason], index): DeckShape[] => {
      const yPt = 168 + index * 104;
      return [
        fill({ xPt: MARGIN_PT, yPt: yPt - 18, widthPt: 832, heightPt: 1 }, "#d9d2c3"),
        words(
          number,
          { xPt: MARGIN_PT, yPt, widthPt: 200, heightPt: 60 },
          { sizePt: 44, bold: true, colour: green, display: true },
        ),
        words(
          reason,
          { xPt: 290, yPt, widthPt: 600, heightPt: 60 },
          { sizePt: 22, bold: false, colour: ink, display: false },
        ),
      ];
    }),
  ];
}

function plansSlide(): DeckShape[] {
  return [
    kicker("THE OFFER", green),
    title("Three plans. Pause any time.", ink),
    ...PLANS.flatMap((plan, index): DeckShape[] => {
      const xPt = MARGIN_PT + index * 284;
      const chosen = index === 1;
      const text = chosen ? paper : ink;
      const box = { xPt, yPt: 150, widthPt: 264, heightPt: 320 };
      const line = (yPt: number) => ({ xPt: xPt + 28, yPt, widthPt: 220, heightPt: 30 });
      return [
        fill(box, chosen ? green : sand),
        words(plan.name.toUpperCase(), line(178), {
          sizePt: 13,
          bold: true,
          colour: chosen ? paper : green,
          display: false,
        }),
        words(
          dollars(plan.dollars),
          { ...line(222), heightPt: 70 },
          { sizePt: 56, bold: true, colour: text, display: true },
        ),
        words("a month", line(296), { sizePt: 15, bold: false, colour: text, display: false }),
        words(`${plan.bags} ${plan.bags === 1 ? "bag" : "bags"} of 340 g`, line(360), {
          sizePt: 17,
          bold: true,
          colour: text,
          display: false,
        }),
        words(plan.for, line(390), { sizePt: 14, bold: false, colour: text, display: false }),
        words(`${planMarginPercent(plan)}% margin`, line(424), {
          sizePt: 13,
          bold: false,
          colour: text,
          display: false,
        }),
      ];
    }),
  ];
}

function boxSlide(): DeckShape[] {
  return [
    {
      kind: "picture",
      picture: DECK_PICTURE.box,
      xPt: 0,
      yPt: 0,
      widthPt: SLIDE.widthPt,
      heightPt: SLIDE.heightPt,
    },
    fill({ xPt: 0, yPt: 470, widthPt: SLIDE.widthPt, heightPt: 70 }, ink),
    words(
      "Every box: roasted Monday, at the door by Thursday.",
      { xPt: MARGIN_PT, yPt: 470, widthPt: 832, heightPt: 70 },
      { sizePt: 22, bold: false, colour: paper, display: false },
    ),
  ];
}

function yearSlide(): DeckShape[] {
  const most = Math.max(...FORECAST.map((month) => month.subscribers));
  const baseline = 470;
  const tallestPt = 270;
  return [
    kicker("THE FIRST YEAR", green),
    title(`${FORECAST[FORECAST.length - 1]?.subscribers} subscribers by next March.`, ink),
    ...FORECAST.flatMap((month, index): DeckShape[] => {
      const heightPt = Math.round((month.subscribers / most) * tallestPt);
      const xPt = MARGIN_PT + 8 + index * 69;
      const last = index === FORECAST.length - 1;
      return [
        fill({ xPt, yPt: baseline - heightPt, widthPt: 44, heightPt }, last ? green : "#9cc2ad"),
        words(
          String(month.subscribers),
          { xPt: xPt - 10, yPt: baseline - heightPt - 26, widthPt: 64, heightPt: 22 },
          { sizePt: 13, bold: last, colour: ink, display: false, align: "center" },
        ),
        words(
          month.month.slice(0, 3),
          { xPt: xPt - 10, yPt: baseline + 8, widthPt: 64, heightPt: 22 },
          { sizePt: 12, bold: false, colour: ink, display: false, align: "center" },
        ),
      ];
    }),
  ];
}

function launchSlide(): DeckShape[] {
  const centre = { xPt: 0, widthPt: SLIDE.widthPt };
  return [
    {
      kind: "picture",
      picture: DECK_PICTURE.badge,
      xPt: 430,
      yPt: 110,
      widthPt: 100,
      heightPt: 100,
    },
    words(
      "Starts April 1",
      { ...centre, yPt: 240, heightPt: 80 },
      { sizePt: 64, bold: true, colour: paper, display: true, align: "center" },
    ),
    words(
      "Pre-orders open March 21 · harborcoffee.example/monthly",
      { ...centre, yPt: 330, heightPt: 30 },
      { sizePt: 18, bold: false, colour: "#9cc2ad", display: false, align: "center" },
    ),
  ];
}

/** The deck, given its pictures' bytes in `DECK_PICTURE` order. */
export function pitchDeck(pictures: Deck["pictures"]): Deck {
  return {
    ...SLIDE,
    ground: paper,
    typeface: TYPE.text,
    accent: green,
    pictures,
    slides: [
      { ground: green, shapes: titleSlide() },
      { shapes: whySlide() },
      { shapes: plansSlide() },
      { ground: ink, shapes: boxSlide() },
      { shapes: yearSlide() },
      { ground: ink, shapes: launchSlide() },
    ],
  };
}
