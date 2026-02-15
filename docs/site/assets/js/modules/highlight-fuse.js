export default function defineFuseLanguage(hljs) {
  const IDENT = "[A-Za-z_][A-Za-z0-9_]*";
  const PATH_IDENT = `${IDENT}(?:\\.${IDENT})*`;
  const USER_TYPE = "[A-Z][A-Za-z0-9_]*";

  const KEYWORDS = [
    "if",
    "else",
    "match",
    "for",
    "in",
    "while",
    "break",
    "continue",
    "return",
    "import",
    "from",
    "as",
    "spawn",
    "await",
    "fn",
    "type",
    "enum",
    "config",
    "service",
    "app",
    "migration",
    "table",
    "test",
    "let",
    "var",
    "body",
    "without",
    "box",
    "at",
    "get",
    "post",
    "put",
    "patch",
    "delete",
    "and",
    "or",
  ].join(" ");

  const LITERALS = "true false null Ok Err Some None";
  const BUILTIN_TYPES = "Int Float Bool String Id Email Bytes Error Unit Html Option Result List Map Task Range";
  const BUILTINS = [
    "print",
    "log",
    "assert",
    "env",
    "serve",
    "asset",
    "svg.inline",
    "html.text",
    "html.raw",
    "html.node",
    "html.render",
    "db.exec",
    "db.query",
    "db.one",
    "db.from",
    "query.select",
    "query.where",
    "query.order_by",
    "query.limit",
    "query.one",
    "query.all",
    "query.exec",
    "query.sql",
    "query.params",
    "task.id",
    "task.done",
    "task.cancel",
  ].join(" ");

  return {
    name: "FUSE",
    aliases: ["fuse"],
    keywords: {
      keyword: KEYWORDS,
      literal: LITERALS,
      type: BUILTIN_TYPES,
      built_in: BUILTINS,
    },
    contains: [
      {
        className: "comment",
        match: /##.*$/,
      },
      {
        className: "comment",
        match: /#.*$/,
      },
      {
        className: "string",
        begin: /"/,
        end: /"/,
        contains: [
          hljs.BACKSLASH_ESCAPE,
          {
            className: "meta",
            begin: /\$\{/,
            end: /\}/,
          },
        ],
      },
      {
        className: "number",
        begin: /\b\d+\.\d+\b/,
      },
      {
        className: "number",
        begin: /\b\d+\b/,
      },
      {
        match: new RegExp(`^(\\s*)(fn)\\s+(${PATH_IDENT})`),
        scope: {
          2: "keyword",
          3: "title.function",
        },
      },
      {
        match: new RegExp(`^(\\s*)(type|enum|config|migration|table|service)\\s+(${PATH_IDENT})`),
        scope: {
          2: "keyword",
          3: "type",
        },
      },
      {
        match: /^\s*(app|test)\s+("[^"]*")/,
        scope: {
          1: "keyword",
          2: "string",
        },
      },
      {
        className: "keyword",
        match: /^\s*(get|post|put|patch|delete)\b/,
      },
      {
        className: "title.function",
        begin: new RegExp(`\\b${PATH_IDENT}(?=\\s*\\()`),
        relevance: 0,
      },
      {
        className: "type",
        begin: new RegExp(`\\b${USER_TYPE}\\b`),
        relevance: 0,
      },
      {
        className: "meta",
        begin: /@[A-Za-z_][A-Za-z0-9_]*/,
      },
      {
        className: "operator",
        begin: /\?\!|\?\?|\?\.|\?\[|->|=>|\.\.|==|!=|<=|>=|[+\-*/%]|=|>|<|!/,
      },
    ],
  };
}
