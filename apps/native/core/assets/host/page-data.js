// What a page asks a SQLite database of its own folder with (`database.rs`, answered by the host
// at `__nr/sql`). window.__nr.sql(file, sql, values) resolves to { columns, rows, changes,
// lastInsertRowid }: rows are objects by column name, values fill the statement's `?`s in
// order. window.__nr.sql(file, [[sql, values], …]) runs several statements as one transaction
// and resolves to one answer each. `file` is a path from the page, as a link's is. A live page
// reads and writes, and its first write makes a database that is not there; a film's scene page
// only reads. A failure rejects with SQLite's own words. The window hears "nr:change" when a
// database the page asked is changed by anything else.
(function () {
  var nr = window.__nr || (window.__nr = {});
  nr.sql = function (file, sql, values) {
    var several = Array.isArray(sql);
    var run = several
      ? sql.map(function (one) {
          return Array.isArray(one) ? [one[0], one[1] || []] : [one, []];
        })
      : [[sql, values || []]];
    var call = { file: decodeURIComponent(new URL(file, location.href).pathname), run: run };
    return fetch("/__nr/sql?q=" + encodeURIComponent(JSON.stringify(call)))
      .then(function (response) {
        return response.json();
      })
      .then(function (answer) {
        if (answer.error) throw new Error(answer.error);
        return several ? answer.results : answer.results[0];
      });
  };
})();
