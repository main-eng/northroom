// What an HTML file of the drive run as a live page (HtmlPageHost) reports to the host: its
// script errors and files that would not load, in its own words. The shell defines
// window.__nrPost(message).
addEventListener(
  "error",
  function (e) {
    try {
      window.__nrPost({
        type: "error",
        text:
          String(e.message || "a file did not load") +
          (e.filename ? " (" + e.filename + ":" + e.lineno + ")" : ""),
      });
    } catch (error) {
      console.error("The page error could not be sent to its host", error);
    }
  },
  true,
);
addEventListener("unhandledrejection", function (e) {
  try {
    window.__nrPost({ type: "error", text: "unhandled rejection: " + String(e.reason) });
  } catch (error) {
    console.error("The page rejection could not be sent to its host", error);
  }
});
