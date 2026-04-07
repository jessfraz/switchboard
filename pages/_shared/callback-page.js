(() => {
  const configScript = document.getElementById("page-config");
  const app = document.getElementById("app");
  if (!configScript || !app) {
    return;
  }

  const config = JSON.parse(configScript.textContent || "{}");
  const theme = config.theme || {};

  document.title = config.title;
  document.documentElement.style.setProperty("--blue", theme.blue || "#4a5ced");
  document.documentElement.style.setProperty("--blue-light", theme.blueLight || "#e8eafd");
  document.documentElement.style.setProperty("--blue-dark", theme.blueDark || "#3d4fd4");
  document.documentElement.style.setProperty("--page-width", theme.pageWidth || "520px");
  document.documentElement.style.setProperty("--field-min-height", theme.fieldMinHeight || "4rem");

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
        <button id="copy-command" type="button">${config.commandButtonLabel}</button>
      </div>
    </section>

    <details>
      <summary>If the terminal is gone</summary>
      <div class="details-body">
        <textarea id="exchange-command" readonly></textarea>
      </div>
    </details>

    <details>
      <summary>Debug details</summary>
      <div class="meta">
        <div class="meta-row">
          <span class="meta-key">Redirect URI</span>
          <span id="redirect-uri" class="meta-value">--</span>
        </div>
        <div class="meta-row">
          <span class="meta-key">Authorization code</span>
          <span id="code-status" class="meta-value">Missing</span>
        </div>
        <div class="meta-row">
          <span class="meta-key">State</span>
          <span id="state-status" class="meta-value">Missing</span>
        </div>
        <div class="meta-row">
          <span class="meta-key">OAuth error</span>
          <span id="error-status" class="meta-value">None</span>
        </div>
      </div>
    </details>

    <p class="footnote">
      The callback query is scrubbed from the address bar after load.
      Authorization codes don't belong in browser history.
    </p>
  `;

  const statusPill = document.getElementById("status-pill");
  const lead = document.getElementById("lead");
  const callbackValueField = document.getElementById("callback-value");
  const exchangeCommand = document.getElementById("exchange-command");
  const redirectUri = document.getElementById("redirect-uri");
  const codeStatus = document.getElementById("code-status");
  const stateStatus = document.getElementById("state-status");
  const errorStatus = document.getElementById("error-status");
  const copyPrimary = document.getElementById("copy-primary");
  const copyCommand = document.getElementById("copy-command");

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

  function render() {
    if (!callbackUrl) {
      statusPill.textContent = "No callback data";
      statusPill.className = "status";
      lead.innerHTML = config.leadNoCallbackHtml;
      callbackValueField.value = "";
      callbackValueField.placeholder = config.placeholder;
      exchangeCommand.value = "";
      exchangeCommand.placeholder = config.placeholder;
      redirectUri.textContent = window.location.origin + window.location.pathname;
      codeStatus.textContent = "Missing";
      stateStatus.textContent = "Missing";
      errorStatus.textContent = "None";
      copyPrimary.textContent = config.copyButtonLabel;
      return;
    }

    const parsed = new URL(callbackUrl);
    const code = parsed.searchParams.get("code");
    const state = parsed.searchParams.get("state");
    const error = parsed.searchParams.get("error");
    const errorDescription = parsed.searchParams.get("error_description");
    const primaryValue = collectPayload(parsed);
    const status = error ? "error" : (hasRequiredParams(parsed) ? "ready" : "incomplete");

    redirectUri.textContent = parsed.origin + parsed.pathname;
    codeStatus.textContent = code ? "Present (" + code.length + " chars)" : "Missing";
    stateStatus.textContent = state ? "Present (" + state.length + " chars)" : "Missing";
    errorStatus.textContent = error ? error + (errorDescription ? ": " + errorDescription : "") : "None";
    callbackValueField.value = primaryValue;
    exchangeCommand.value = config.commandPrefix + " " + shellQuote(commandValue(primaryValue, callbackUrl));
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
      return;
    }

    statusPill.textContent = "Incomplete callback";
    statusPill.className = "status";
    lead.innerHTML = config.leadIncompleteHtml;
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
  copyCommand.addEventListener("click", () => {
    copyText(exchangeCommand.value, copyCommand, config.commandCopySuccessLabel || "Copied!");
  });

  render();
})();
