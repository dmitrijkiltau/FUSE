export default function defineFuseLanguage(hljs) {
  const IDENT = "[A-Za-z_][A-Za-z0-9_]*";
  const KEYWORDS = [
    "app",
    "as",
    "box",
    "break",
    "case",
    "config",
    "continue",
    "db",
    "else",
    "enum",
    "fn",
    "for",
    "if",
    "import",
    "in",
    "match",
    "migration",
    "return",
    "service",
    "spawn",
    "test",
    "type",
    "var",
    "while",
  ].join(" ");
  const LITERALS = "true false null";
  const BUILTINS = [
    "assert",
    "db.exec",
    "db.from",
    "db.one",
    "db.query",
    "env",
    "json",
    "log",
    "now",
    "openapi",
    "print",
    "query.all",
    "query.exec",
    "query.limit",
    "query.one",
    "query.order_by",
    "query.params",
    "query.select",
    "query.sql",
    "query.where",
    "range",
    "read",
    "serve",
    "task.await",
    "task.id",
    "task.is_done",
    "task.result",
    "task.status",
    "write",
  ].join(" ");

  return {
    name: "FUSE",
    aliases: ["fuse"],
    keywords: {
      keyword: KEYWORDS,
      literal: LITERALS,
      built_in: BUILTINS,
    },
    contains: [
      hljs.COMMENT("#", "$"),
      hljs.QUOTE_STRING_MODE,
      {
        className: "string",
        begin: /"""/,
        end: /"""/,
      },
      {
        className: "number",
        begin: /\b\d+(\.\d+)?\b/,
      },
      {
        className: "type",
        begin: "\\b" + IDENT + "(\\." + IDENT + ")*(\\[[^\\]]+\\])?\\b",
        relevance: 0,
      },
      {
        className: "title.function",
        begin: "\\bfn\\s+" + IDENT,
        returnBegin: true,
        contains: [
          {
            begin: "\\bfn\\b",
            className: "keyword",
          },
          {
            begin: IDENT,
            className: "title.function",
            relevance: 0,
          },
        ],
      },
      {
        className: "meta",
        begin: /@[A-Za-z_][A-Za-z0-9_]*/,
      },
      {
        className: "operator",
        begin: /->|=>|:=|\?!|\?\?|\?\.|==|!=|<=|>=|\+\+|--|\.\./,
      },
    ],
  };
}
