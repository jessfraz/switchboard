(() => {
  const configScript = document.getElementById("page-config");
  const app = document.getElementById("app");
  if (!configScript || !app) {
    return;
  }

  const config = JSON.parse(configScript.textContent || "{}");
  const theme = config.theme || {};
  const showCommandSection = config.showCommandSection !== false && !!config.commandPrefix;
  const localBridge = config.localBridge || {};
  const metaFields = Array.isArray(config.metaFields) && config.metaFields.length > 0 ? config.metaFields : [
    { kind: "redirect_uri", label: "Redirect URI" },
    { kind: "param_presence", label: "Authorization code", param: "code" },
    { kind: "param_presence", label: "State", param: "state" },
    {
      kind: "error",
      label: "OAuth error",
      param: "error",
      descriptionParam: "error_description"
    }
  ];

  document.title = config.title;
  document.documentElement.style.setProperty("--blue", theme.blue || "#4a5ced");
  document.documentElement.style.setProperty("--blue-light", theme.blueLight || "#e8eafd");
  document.documentElement.style.setProperty("--blue-dark", theme.blueDark || "#3d4fd4");
  document.documentElement.style.setProperty("--page-width", theme.pageWidth || "520px");
  document.documentElement.style.setProperty("--field-min-height", theme.fieldMinHeight || "4rem");

  const metaRowsHtml = metaFields
    .map((field, index) => `
        <div class="meta-row">
          <span class="meta-key">${field.label}</span>
          <span data-meta-index="${index}" class="meta-value">--</span>
        </div>
      `)
    .join("");

  app.innerHTML = `
    <header>
      <div id="status-pill" class="status">Waiting for callback</div>
      <h1>${config.title}</h1>
      <p id="lead">${config.leadInitialHtml}</p>
    </header>

    <section>
      <label for="callback-value">${config.fieldLabel}</label>
      <textarea id="callback-value" readonly></textarea>
      <div class="actions">
        <button id="copy-primary" class="primary" type="button">${config.copyButtonLabel}</button>
        ${showCommandSection ? `<button id="copy-command" type="button">${config.commandButtonLabel}</button>` : ""}
      </div>
    </section>

    ${showCommandSection ? `
      <details>
        <summary>If the terminal is gone</summary>
        <div class="details-body">
          <textarea id="exchange-command" readonly></textarea>
        </div>
      </details>
    ` : ""}

    <details>
      <summary>Debug details</summary>
      <div class="meta">
        ${metaRowsHtml}
      </div>
    </details>

    <p class="footnote">
      The callback query is scrubbed from the address bar after load.
      OAuth callbacks don't belong in browser history.
    </p>
  `;

  const statusPill = document.getElementById("status-pill");
  const lead = document.getElementById("lead");
  const callbackValueField = document.getElementById("callback-value");
  const exchangeCommand = document.getElementById("exchange-command");
  const copyPrimary = document.getElementById("copy-primary");
  const copyCommand = document.getElementById("copy-command");
  const metaValues = Array.from(document.querySelectorAll("[data-meta-index]"));

  const currentUrl = new URL(window.location.href);
  const hasQuery = currentUrl.search.length > 1;
  const callbackUrl = hasQuery ? currentUrl.toString() : window.sessionStorage.getItem(config.storageKey);

  if (hasQuery) {
    window.sessionStorage.setItem(config.storageKey, callbackUrl);
    window.history.replaceState({}, document.title, currentUrl.origin + currentUrl.pathname);
  }

  function shellQuote(value) {
    return "'" + value.replace(/'/g, "'\"'\"'") + "'";
  }

  function collectPayload(parsed) {
    if (config.primaryValueMode === "callback_url") {
      return parsed.toString();
    }

    const code = parsed.searchParams.get("code");
    if (config.preferCodeOnly && code) {
      return code;
    }

    const params = new URLSearchParams();
    (config.payloadKeys || []).forEach((key) => {
      const value = parsed.searchParams.get(key);
      if (value) {
        params.set(key, value);
      }
    });
    return params.toString();
  }

  function collectBridgePayload(parsed) {
    const params = new URLSearchParams();
    (config.payloadKeys || []).forEach((key) => {
      const value = parsed.searchParams.get(key);
      if (value) {
        params.set(key, value);
      }
    });
    return params.toString();
  }

  function commandValue(primaryValue, callbackValue) {
    if (config.commandValueMode === "callback_url") {
      return callbackValue;
    }
    if (config.commandValueMode === "primary_value") {
      return primaryValue;
    }
    return primaryValue || callbackValue;
  }

  function hasRequiredParams(parsed) {
    return (config.readyParams || []).every((key) => {
      const value = parsed.searchParams.get(key);
      return value && value.length > 0;
    });
  }

  function primaryButtonLabel(status) {
    if (status === "error") {
      return config.copyButtonLabelError || config.copyButtonLabel;
    }
    if (status === "incomplete") {
      return config.copyButtonLabelIncomplete || config.copyButtonLabel;
    }
    return config.copyButtonLabel;
  }

  function metaFieldText(field, parsed) {
    if (field.kind === "redirect_uri") {
      const url = parsed || currentUrl;
      return url.origin + url.pathname;
    }

    if (field.kind === "error") {
      if (!parsed) {
        return "None";
      }
      const error = parsed.searchParams.get(field.param || "error");
      const description = parsed.searchParams.get(field.descriptionParam || "error_description");
      return error ? error + (description ? ": " + description : "") : "None";
    }

    const value = parsed ? parsed.searchParams.get(field.param || "") : null;
    if (!value) {
      return field.missingLabel || "Missing";
    }

    if (field.kind === "param_value") {
      return value;
    }

    return "Present (" + value.length + " chars)";
  }

  function renderMeta(parsed) {
    metaFields.forEach((field, index) => {
      const metaValue = metaValues[index];
      if (metaValue) {
        metaValue.textContent = metaFieldText(field, parsed);
      }
    });
  }

  function render() {
    if (!callbackUrl) {
      statusPill.textContent = "No callback data";
      statusPill.className = "status";
      lead.innerHTML = config.leadNoCallbackHtml;
      callbackValueField.value = "";
      callbackValueField.placeholder = config.placeholder;
      if (exchangeCommand) {
        exchangeCommand.value = "";
        exchangeCommand.placeholder = config.placeholder;
      }
      renderMeta(null);
      copyPrimary.textContent = config.copyButtonLabel;
      return;
    }

    const parsed = new URL(callbackUrl);
    const error = parsed.searchParams.get("error");
    const primaryValue = collectPayload(parsed);
    const status = error ? "error" : (hasRequiredParams(parsed) ? "ready" : "incomplete");

    renderMeta(parsed);
    callbackValueField.value = primaryValue;
    if (exchangeCommand) {
      exchangeCommand.value = config.commandPrefix + " " + shellQuote(commandValue(primaryValue, callbackUrl));
    }
    copyPrimary.textContent = primaryButtonLabel(status);

    if (status === "error") {
      statusPill.textContent = "OAuth error";
      statusPill.className = "status bad";
      lead.innerHTML = config.leadErrorHtml;
      return;
    }

    if (status === "ready") {
      statusPill.textContent = "Ready";
      statusPill.className = "status good";
      lead.innerHTML = config.leadReadyHtml;
      notifyLocalBridge(parsed);
      return;
    }

    statusPill.textContent = "Incomplete callback";
    statusPill.className = "status";
    lead.innerHTML = config.leadIncompleteHtml;
  }

  async function notifyLocalBridge(parsed) {
    const urls = Array.isArray(localBridge.urls) ? localBridge.urls : [];
    if (!urls.length) {
      return;
    }

    const payload = collectBridgePayload(parsed);
    if (!payload) {
      return;
    }
    if (window.sessionStorage.getItem(config.storageKey + "_bridge_sent") === payload) {
      return;
    }

    for (const baseUrl of urls) {
      try {
        const response = await fetch(baseUrl + "?" + payload, {
          method: "GET",
          mode: "cors",
          cache: "no-store",
          referrerPolicy: "no-referrer",
          targetAddressSpace: "local",
        });
        if (!response.ok) {
          throw new Error("local bridge rejected callback");
        }
        window.sessionStorage.setItem(config.storageKey + "_bridge_sent", payload);
        statusPill.textContent = localBridge.statusLabel || "Sent to CLI";
        statusPill.className = "status good";
        return;
      } catch (error) {
        // The terminal fallback is still right there. No need to turn this into drama.
      }
    }
  }

  async function copyText(text, button, successLabel) {
    const originalLabel = button.textContent;
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = successLabel;
    } catch (error) {
      button.textContent = "Copy failed";
    }
    window.setTimeout(() => {
      button.textContent = originalLabel;
    }, 1500);
  }

  copyPrimary.addEventListener("click", () => {
    copyText(callbackValueField.value, copyPrimary, config.copySuccessLabel || "Copied!");
  });
  if (copyCommand && exchangeCommand) {
    copyCommand.addEventListener("click", () => {
      copyText(exchangeCommand.value, copyCommand, config.commandCopySuccessLabel || "Copied!");
    });
  }

  render();
})();
