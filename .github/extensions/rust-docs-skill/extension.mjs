import { joinSession } from "@github/copilot-sdk/extension";

const DEFAULT_MAX_CHARS = 12000;
const MIN_MAX_CHARS = 1000;
const MAX_MAX_CHARS = 40000;

function clampMaxChars(value) {
    const n = Number(value);
    if (!Number.isFinite(n)) return DEFAULT_MAX_CHARS;
    return Math.max(MIN_MAX_CHARS, Math.min(MAX_MAX_CHARS, Math.floor(n)));
}

function decodeHtmlEntities(text) {
    return text
        .replace(/&nbsp;/g, " ")
        .replace(/&amp;/g, "&")
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&quot;/g, '"')
        .replace(/&#39;/g, "'");
}

function htmlToText(html) {
    const mainMatch = html.match(/<main[\s\S]*?<\/main>/i);
    let content = mainMatch ? mainMatch[0] : html;

    content = content
        .replace(/<script[\s\S]*?<\/script>/gi, " ")
        .replace(/<style[\s\S]*?<\/style>/gi, " ")
        .replace(/<noscript[\s\S]*?<\/noscript>/gi, " ")
        .replace(/<[^>]+>/g, " ");

    content = decodeHtmlEntities(content)
        .replace(/[ \t]+\n/g, "\n")
        .replace(/\n[ \t]+/g, "\n")
        .replace(/\n{3,}/g, "\n\n")
        .replace(/[ \t]{2,}/g, " ")
        .trim();

    return content;
}

async function fetchDoc(url, maxChars) {
    const response = await fetch(url, {
        headers: {
            "User-Agent": "copilot-cli-rust-docs-skill",
            Accept: "text/html,application/xhtml+xml",
        },
    });

    if (!response.ok) {
        throw new Error(`HTTP ${response.status} fetching ${url}`);
    }

    const html = await response.text();
    const text = htmlToText(html);
    return text.slice(0, maxChars);
}

function buildStdUrl({ mode, target }) {
    const trimmed = String(target || "").trim();
    if (!trimmed) {
        return "https://doc.rust-lang.org/std/index.html";
    }

    if (mode === "search") {
        return `https://doc.rust-lang.org/std/index.html?search=${encodeURIComponent(trimmed)}`;
    }

    if (/^https?:\/\//i.test(trimmed)) {
        if (!trimmed.startsWith("https://doc.rust-lang.org/std/")) {
            throw new Error("Only https://doc.rust-lang.org/std/ URLs are allowed.");
        }
        return trimmed;
    }

    const clean = trimmed.replace(/^\/+/, "");
    return `https://doc.rust-lang.org/std/${clean}`;
}

function buildDocsRsUrl({ crateName, version, itemPath }) {
    const crateTrimmed = String(crateName || "").trim();
    if (!crateTrimmed) {
        throw new Error("crate is required");
    }
    const ver = String(version || "latest").trim() || "latest";
    const base = `https://docs.rs/${encodeURIComponent(crateTrimmed)}/${encodeURIComponent(ver)}/${encodeURIComponent(crateTrimmed)}/`;

    const item = String(itemPath || "").trim();
    if (!item) return base;
    return `${base}${item.replace(/^\/+/, "")}`;
}

const session = await joinSession({
    tools: [
        {
            name: "read_rust_std_docs",
            description:
                "Read Rust standard library docs from doc.rust-lang.org/std (page mode or search mode).",
            parameters: {
                type: "object",
                properties: {
                    mode: {
                        type: "string",
                        enum: ["page", "search"],
                        description: "Use 'page' for a specific std page path/URL, or 'search' for std docs search.",
                        default: "search",
                    },
                    target: {
                        type: "string",
                        description:
                            "For page mode: std page path like 'vec/struct.Vec.html' or full std URL. For search mode: search query like 'Vec::retain'.",
                    },
                    maxChars: {
                        type: "integer",
                        description: "Maximum characters returned from extracted page text (1000-40000).",
                        default: DEFAULT_MAX_CHARS,
                    },
                },
            },
            skipPermission: true,
            handler: async (args) => {
                try {
                    const maxChars = clampMaxChars(args?.maxChars);
                    const mode = args?.mode === "page" ? "page" : "search";
                    const url = buildStdUrl({ mode, target: args?.target });
                    const body = await fetchDoc(url, maxChars);
                    return `URL: ${url}\n\n${body}`;
                } catch (error) {
                    return {
                        resultType: "failure",
                        textResultForLlm: `read_rust_std_docs failed: ${error instanceof Error ? error.message : String(error)}`,
                    };
                }
            },
        },
        {
            name: "read_docs_rs",
            description: "Read crate docs from docs.rs.",
            parameters: {
                type: "object",
                properties: {
                    crate: {
                        type: "string",
                        description: "Crate name (for example: serde, tokio, ratatui).",
                    },
                    version: {
                        type: "string",
                        description: "Crate version, defaults to 'latest'.",
                        default: "latest",
                    },
                    itemPath: {
                        type: "string",
                        description:
                            "Optional path under crate docs, e.g. 'struct.Serializer.html' or 'de/trait.DeserializeOwned.html'.",
                    },
                    maxChars: {
                        type: "integer",
                        description: "Maximum characters returned from extracted page text (1000-40000).",
                        default: DEFAULT_MAX_CHARS,
                    },
                },
                required: ["crate"],
            },
            skipPermission: true,
            handler: async (args) => {
                try {
                    const maxChars = clampMaxChars(args?.maxChars);
                    const url = buildDocsRsUrl({
                        crateName: args?.crate,
                        version: args?.version,
                        itemPath: args?.itemPath,
                    });
                    const body = await fetchDoc(url, maxChars);
                    return `URL: ${url}\n\n${body}`;
                } catch (error) {
                    return {
                        resultType: "failure",
                        textResultForLlm: `read_docs_rs failed: ${error instanceof Error ? error.message : String(error)}`,
                    };
                }
            },
        },
    ],
});

await session.log("rust-docs-skill loaded: read_rust_std_docs, read_docs_rs");
