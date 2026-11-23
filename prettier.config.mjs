/** @type {import("prettier").Config} */
export default {
  printWidth: 100,
  tabWidth: 2,
  useTabs: false,
  singleQuote: false,
  trailingComma: "all",
  semi: true,
  endOfLine: "lf",
  proseWrap: "preserve",
  // Keep the author's key quoting. `as-needed` would rewrite every quoted key in the repo.
  quoteProps: "preserve",
};
