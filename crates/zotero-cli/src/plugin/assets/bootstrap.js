/* CLI Bridge for Zotero (Rust Fork) — bootstrap plugin
 *
 * Registers a POST /cli-bridge/eval endpoint on Zotero's built-in HTTP server
 * so that external CLI tools can execute privileged JavaScript without GUI
 * automation.
 *
 * Works on macOS, Windows, and Linux — any platform that runs Zotero 7+ through 10+.
 */

var cliBridgeEndpoint;
var cliOwnershipEndpoint;

const ADDON_ID = "cli-bridge@cli-anything-rust.dev";
const ADDON_VERSION = "1.2.1";

function _serializeError(e) {
  var message = null;
  if (e == null) {
    message = "unknown error";
  } else if (typeof e === "string") {
    message = e;
  } else if (typeof e === "number" || typeof e === "boolean") {
    message = String(e);
  } else {
    message =
      (e && (e.message || e.name || (e.toString && e.toString()))) ||
      String(e);
  }
  // Avoid empty / undefined messages which used to collapse to error: "{}"
  if (!message || message === "undefined" || message === "[object Object]") {
    try {
      message = JSON.stringify(e);
    } catch (_jsonErr) {
      message = "unknown error";
    }
  }
  return {
    error: message,
    name: (e && e.name) || null,
    stack: (e && e.stack) ? String(e.stack).slice(0, 2000) : null,
    raw: String(e),
  };
}

function startup({ id, version, rootURI }) {
  cliBridgeEndpoint = function () {};
  cliBridgeEndpoint.prototype = {
    supportedMethods: ["POST"],
    supportedDataTypes: ["text/plain"],
    permitBookmarklet: false,
    init: async function (options) {
      try {
        if (options.data === "return 'ping';" || options.data === "__PING__" || options.data === "__OWNERSHIP__") {
          return [200, "application/json", JSON.stringify({
            pong: true,
            fork: "zotero-rust-cli",
            id: id || ADDON_ID,
            version: version || ADDON_VERSION,
            ownership: "verified"
          })];
        }
        var result = await eval("(async () => {" + options.data + "})()");
        // undefined is not valid JSON; normalize to null for clients
        if (typeof result === "undefined") {
          result = null;
        }
        return [200, "application/json", JSON.stringify(result)];
      } catch (e) {
        return [500, "application/json", JSON.stringify(_serializeError(e))];
      }
    },
  };
  Zotero.Server.Endpoints["/cli-bridge/eval"] = cliBridgeEndpoint;

  cliOwnershipEndpoint = function () {};
  cliOwnershipEndpoint.prototype = {
    supportedMethods: ["GET", "POST"],
    supportedDataTypes: ["text/plain", "application/json"],
    permitBookmarklet: false,
    init: async function (_options) {
      return [200, "application/json", JSON.stringify({
        fork: "zotero-rust-cli",
        id: id || ADDON_ID,
        version: version || ADDON_VERSION,
        ownership: "verified"
      })];
    },
  };
  Zotero.Server.Endpoints["/cli-bridge/ownership"] = cliOwnershipEndpoint;

  Zotero.debug("[CLI Bridge] /cli-bridge/eval and /cli-bridge/ownership endpoints registered");
}

function shutdown() {
  delete Zotero.Server.Endpoints["/cli-bridge/eval"];
  delete Zotero.Server.Endpoints["/cli-bridge/ownership"];
  cliBridgeEndpoint = null;
  cliOwnershipEndpoint = null;
  Zotero.debug("[CLI Bridge] /cli-bridge endpoints removed");
}

function install() {}
function uninstall() {}
