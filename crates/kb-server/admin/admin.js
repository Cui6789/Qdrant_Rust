(() => {
  "use strict";

  const API_ROOT = "/admin/api";
  const DOCUMENT_PAGE_SIZE = 25;
  const CHUNK_PAGE_SIZE = 25;
  const SETTINGS_SECTIONS = {
    embedding: {
      label: "嵌入",
      provider: "embedding_provider",
      modes: {
        local: ["embedding_local_model", "embedding_local_dimensions"],
        api: ["embedding_api_protocol", "embedding_api_base_url", "embedding_api_model", "embedding_api_dimensions", "embedding_api_key"],
        mock: ["embedding_mock_dimensions"],
      },
    },
    rerank: {
      label: "重排",
      provider: "rerank_provider",
      modes: {
        disabled: [],
        local: ["rerank_local_model", "rerank_candidate_limit"],
        api: ["rerank_api_endpoint", "rerank_api_model", "rerank_api_key", "rerank_candidate_limit"],
      },
    },
    parsing: {
      label: "解析",
      provider: "parsing_provider",
      modes: {
        native: [],
        mineru_local: ["mineru_local_url"],
        mineru_cloud: ["mineru_cloud_url", "mineru_model_version", "mineru_api_key"],
      },
    },
  };
  const INTEGER_SETTINGS = {
    embedding_local_dimensions: { min: 0, max: null, label: "本地输出维度" },
    embedding_api_dimensions: { min: 0, max: null, label: "API 维度" },
    embedding_mock_dimensions: { min: 1, max: 4096, label: "Mock 维度" },
    rerank_candidate_limit: { min: 1, max: 100, label: "重排候选数量" },
  };
  const EMBEDDING_REINDEX_FIELDS = new Set([
    "embedding_provider",
    "embedding_local_model",
    "embedding_local_dimensions",
    "embedding_api_protocol",
    "embedding_api_base_url",
    "embedding_api_model",
    "embedding_api_dimensions",
    "embedding_mock_dimensions",
  ]);
  const EMBEDDING_PRESETS = {
    openai: {
      label: "OpenAI",
      protocol: "openai",
      baseUrl: "https://api.openai.com/v1",
      model: "text-embedding-3-small",
      dimensions: 0,
    },
    "qwen-maas": {
      label: "通义 MaaS",
      protocol: "openai",
      baseUrl: "https://maas.qianwenaiapi.com/compatible-mode/v1",
      model: "qwen3.7-text-embedding-flash",
      dimensions: 0,
    },
    "qwen-native": {
      label: "通义原生",
      protocol: "dashscope",
      baseUrl: "https://cn-beijing.maas.qianwenaiapi.com/api/v1",
      model: "qwen3.7-text-embedding-flash",
      dimensions: 0,
    },
    bailian: {
      label: "百炼兼容",
      protocol: "openai",
      baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
      model: "text-embedding-v4",
      dimensions: 0,
    },
    siliconflow: {
      label: "SiliconFlow",
      protocol: "openai",
      baseUrl: "https://api.siliconflow.cn/v1",
      model: "BAAI/bge-m3",
      dimensions: 0,
    },
    ollama: {
      label: "Ollama",
      protocol: "openai",
      baseUrl: "http://localhost:11434/v1",
      model: "nomic-embed-text",
      dimensions: 0,
    },
  };

  const state = {
    accessKey: null,
    settings: null,
    collections: [],
    selectedCollection: null,
    documents: [],
    documentCursorStack: [null],
    documentCursorIndex: 0,
    documentNextCursor: null,
    documentRequestToken: 0,
    selectedDocument: null,
    chunks: [],
    chunkNextCursor: null,
    chunkFocusIndex: null,
    chunkRequestToken: 0,
    searchSpec: null,
    searchRequestToken: 0,
    collectionRequestToken: 0,
    confirmResolver: null,
    settingsBusy: false,
    settingsTab: "embedding",
    settingsProbeAbort: null,
  };

  class ApiError extends Error {
    constructor(message, status = 0, options = {}) {
      super(message);
      this.name = "ApiError";
      this.status = status;
      this.sessionExpired = Boolean(options.sessionExpired);
    }
  }

  const elements = {
    authView: byId("authView"),
    adminView: byId("adminView"),
    sessionStatus: byId("sessionStatus"),
    sessionStatusText: byId("sessionStatusText"),
    feedback: byId("feedback"),
    feedbackMessage: byId("feedbackMessage"),
    feedbackDismiss: byId("feedbackDismiss"),
    openSettingsButton: byId("openSettingsButton"),
    logoutButton: byId("logoutButton"),
    signInForm: byId("signInForm"),
    accessKeyInput: byId("accessKeyInput"),
    signInSubmit: byId("signInSubmit"),
    signInError: byId("signInError"),
    collectionsCount: byId("collectionsCount"),
    collectionList: byId("collectionList"),
    collectionsEmpty: byId("collectionsEmpty"),
    refreshCollectionsButton: byId("refreshCollectionsButton"),
    createCollectionForm: byId("createCollectionForm"),
    collectionName: byId("collectionName"),
    createCollectionSubmit: byId("createCollectionSubmit"),
    collectionFormStatus: byId("collectionFormStatus"),
    selectedCollectionName: byId("selectedCollectionName"),
    selectedCollectionMeta: byId("selectedCollectionMeta"),
    selectedCollectionPoints: byId("selectedCollectionPoints"),
    selectedCollectionVector: byId("selectedCollectionVector"),
    refreshDocumentsButton: byId("refreshDocumentsButton"),
    openDocumentDialogButton: byId("openDocumentDialogButton"),
    openUploadDialogButton: byId("openUploadDialogButton"),
    deleteCollectionButton: byId("deleteCollectionButton"),
    documentsStatus: byId("documentsStatus"),
    documentsBody: byId("documentsBody"),
    documentsEmpty: byId("documentsEmpty"),
    documentsPageLabel: byId("documentsPageLabel"),
    documentsPreviousButton: byId("documentsPreviousButton"),
    documentsNextButton: byId("documentsNextButton"),
    searchForm: byId("searchForm"),
    searchQuery: byId("searchQuery"),
    searchTopK: byId("searchTopK"),
    searchDocIds: byId("searchDocIds"),
    searchSubmit: byId("searchSubmit"),
    searchStatus: byId("searchStatus"),
    searchResultsSection: byId("searchResultsSection"),
    searchSummary: byId("searchSummary"),
    searchResultsList: byId("searchResultsList"),
    searchEmpty: byId("searchEmpty"),
    refreshSearchButton: byId("refreshSearchButton"),
    clearSearchButton: byId("clearSearchButton"),
    inspector: byId("inspector"),
    manualDocumentForm: byId("manualDocumentForm"),
    manualDocId: byId("manualDocId"),
    manualDocumentSubmit: byId("manualDocumentSubmit"),
    selectedDocumentSummary: byId("selectedDocumentSummary"),
    selectedDocumentTitle: byId("selectedDocumentTitle"),
    selectedDocumentMeta: byId("selectedDocumentMeta"),
    deleteSelectedDocumentButton: byId("deleteSelectedDocumentButton"),
    chunkStatus: byId("chunkStatus"),
    chunkCountLabel: byId("chunkCountLabel"),
    chunkEmpty: byId("chunkEmpty"),
    chunkList: byId("chunkList"),
    loadMoreChunksButton: byId("loadMoreChunksButton"),
    documentDialog: byId("documentDialog"),
    documentForm: byId("documentForm"),
    documentDocId: byId("documentDocId"),
    documentFormat: byId("documentFormat"),
    documentTitle: byId("documentTitle"),
    documentSource: byId("documentSource"),
    documentTags: byId("documentTags"),
    documentText: byId("documentText"),
    documentFormStatus: byId("documentFormStatus"),
    documentSubmit: byId("documentSubmit"),
    uploadDialog: byId("uploadDialog"),
    uploadForm: byId("uploadForm"),
    uploadFile: byId("uploadFile"),
    uploadDocId: byId("uploadDocId"),
    uploadTitle: byId("uploadTitle"),
    uploadSource: byId("uploadSource"),
    uploadTags: byId("uploadTags"),
    uploadParser: byId("uploadParser"),
    uploadFormStatus: byId("uploadFormStatus"),
    uploadSubmit: byId("uploadSubmit"),
    settingsDialog: byId("settingsDialog"),
    settingsClose: byId("settingsClose"),
    settingsForm: byId("settingsForm"),
    settingsFields: byId("settingsFields"),
    settingsFormStatus: byId("settingsFormStatus"),
    settingsSubmit: byId("settingsSubmit"),
    settingsActionHint: byId("settingsActionHint"),
    settingsDiscard: byId("settingsDiscard"),
    settingsProbe: byId("settingsProbe"),
    settingsStopProbe: byId("settingsStopProbe"),
    rerankActiveSummary: byId("rerankActiveSummary"),
    rerankRouteStep: byId("rerankRouteStep"),
    rerankDisabledFields: byId("rerankDisabledFields"),
    rerankEnabledFields: byId("rerankEnabledFields"),
    rerankLocalFields: byId("rerankLocalFields"),
    rerankApiFields: byId("rerankApiFields"),
    parsingActiveSummary: byId("parsingActiveSummary"),
    parsingRouteInput: byId("parsingRouteInput"),
    parsingRouteStep: byId("parsingRouteStep"),
    parsingNativeFields: byId("parsingNativeFields"),
    parsingLocalFields: byId("parsingLocalFields"),
    parsingCloudFields: byId("parsingCloudFields"),
    parsingKeepLocalNote: byId("parsingKeepLocalNote"),
    clearInactiveMineruKey: byId("clearInactiveMineruKey"),
    settingEmbeddingProviderSummary: byId("settingEmbeddingProviderSummary"),
    settingEmbeddingModel: byId("settingEmbeddingModel"),
    settingEmbeddingDimension: byId("settingEmbeddingDimension"),
    settingEmbeddingProviderHelp: byId("settingEmbeddingProviderHelp"),
    settingEmbeddingLocalFields: byId("settingEmbeddingLocalFields"),
    settingEmbeddingLocalDimensionsHelp: byId("settingEmbeddingLocalDimensionsHelp"),
    settingEmbeddingApiFields: byId("settingEmbeddingApiFields"),
    settingEmbeddingMockFields: byId("settingEmbeddingMockFields"),
    settingEmbeddingApiProtocolHint: byId("settingEmbeddingApiProtocolHint"),
    testEmbeddingButton: byId("testEmbeddingButton"),
    restoreEmbeddingButton: byId("restoreEmbeddingButton"),
    embeddingResultDialog: byId("embeddingResultDialog"),
    embeddingResultKicker: byId("embeddingResultKicker"),
    embeddingResultTitle: byId("embeddingResultTitle"),
    embeddingResultMessage: byId("embeddingResultMessage"),
    embeddingResultLatency: byId("embeddingResultLatency"),
    embeddingResultModel: byId("embeddingResultModel"),
    embeddingResultDimension: byId("embeddingResultDimension"),
    embeddingResultNorm: byId("embeddingResultNorm"),
    embeddingProbeChart: byId("embeddingProbeChart"),
    embeddingProbeSampleMeta: byId("embeddingProbeSampleMeta"),
    embeddingProbeBars: byId("embeddingProbeBars"),
    confirmDialog: byId("confirmDialog"),
    confirmDialogTitle: byId("confirmDialogTitle"),
    confirmDialogMessage: byId("confirmDialogMessage"),
    confirmCancelButton: byId("confirmCancelButton"),
    confirmAcceptButton: byId("confirmAcceptButton"),
  };

  const settingsControls = {
    embedding_provider: byId("settingEmbeddingProvider"),
    embedding_local_model: byId("settingEmbeddingLocalModel"),
    embedding_local_dimensions: byId("settingEmbeddingLocalDimensions"),
    embedding_api_protocol: byId("settingEmbeddingApiProtocol"),
    embedding_api_base_url: byId("settingEmbeddingApiBaseUrl"),
    embedding_api_model: byId("settingEmbeddingApiModel"),
    embedding_api_dimensions: byId("settingEmbeddingApiDimensions"),
    embedding_mock_dimensions: byId("settingEmbeddingMockDimensions"),
    rerank_candidate_limit: byId("settingRerankCandidateLimit"),
    rerank_local_model: byId("settingRerankLocalModel"),
    rerank_api_endpoint: byId("settingRerankApiEndpoint"),
    rerank_api_model: byId("settingRerankApiModel"),
    mineru_local_url: byId("settingMineruLocalUrl"),
    mineru_cloud_url: byId("settingMineruCloudUrl"),
    mineru_model_version: byId("settingMineruModelVersion"),
  };

  const secretControls = [
    {
      field: "embedding_api_key",
      input: byId("settingEmbeddingApiKey"),
      clear: byId("settingEmbeddingApiKeyClear"),
      status: byId("settingEmbeddingApiKeySet"),
      statusField: "embedding_api_key_set",
    },
    {
      field: "rerank_api_key",
      input: byId("settingRerankApiKey"),
      clear: byId("settingRerankApiKeyClear"),
      status: byId("settingRerankApiKeySet"),
      statusField: "rerank_api_key_set",
    },
    {
      field: "mineru_api_key",
      input: byId("settingMineruApiKey"),
      clear: byId("settingMineruApiKeyClear"),
      status: byId("settingMineruApiKeySet"),
      statusField: "mineru_api_key_set",
    },
  ];

  const settingsRadios = Object.fromEntries(
    ["rerank_provider", "parsing_provider"].map((field) => [
      field, [...elements.settingsForm.querySelectorAll(`input[name="${field}"]`)],
    ]),
  );
  const settingsTabs = [...elements.settingsForm.querySelectorAll("[data-settings-tab]")];

  function byId(id) {
    const element = document.getElementById(id);
    if (!element) {
      throw new Error(`缺少页面元素：${id}`);
    }
    return element;
  }

  function createTextElement(tagName, className, text) {
    const element = document.createElement(tagName);
    if (className) {
      element.className = className;
    }
    if (text !== undefined) {
      element.textContent = String(text);
    }
    return element;
  }

  function valueOrDash(value) {
    if (value === null || value === undefined || value === "") {
      return "—";
    }
    if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
      return String(value);
    }
    try {
      return JSON.stringify(value);
    } catch {
      return "—";
    }
  }

  function formatHeadingPath(value) {
    if (Array.isArray(value)) {
      return value.map((part) => valueOrDash(part)).join(" / ");
    }
    return valueOrDash(value);
  }

  function formatDate(value) {
    if (value === null || value === undefined || value === "") {
      return "—";
    }
    const raw = String(value);
    const date = new Date(raw);
    if (Number.isNaN(date.getTime())) {
      return raw;
    }
    return new Intl.DateTimeFormat("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  }

  function formatJson(value) {
    if (value === null || value === undefined || value === "") {
      return "—";
    }
    if (typeof value === "string") {
      return value;
    }
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return "—";
    }
  }

  function parseCommaList(value) {
    return value
      .split(/[,\uFF0C]/)
      .map((part) => part.trim())
      .filter(Boolean);
  }

  function parseIdList(value) {
    return Array.from(
      new Set(
        value
          .split(/[\n,\uFF0C]/)
          .map((part) => part.trim())
          .filter(Boolean),
      ),
    );
  }

  function setStatus(element, message, tone = "info") {
    element.textContent = message;
    if (tone === "info") {
      delete element.dataset.tone;
    } else {
      element.dataset.tone = tone;
    }
  }

  function showFeedback(message, tone = "info", assertive = false) {
    elements.feedback.hidden = false;
    elements.feedback.dataset.tone = tone;
    elements.feedback.setAttribute("role", assertive ? "alert" : "status");
    elements.feedback.setAttribute("aria-live", assertive ? "assertive" : "polite");
    elements.feedbackMessage.textContent = message;
  }

  function setSessionStatus(label, status = "idle") {
    elements.sessionStatusText.textContent = label;
    elements.sessionStatus.dataset.state = status;
  }

  function setButtonBusy(button, busy) {
    if (busy) {
      if (!button.dataset.idleLabel) {
        button.dataset.idleLabel = button.textContent;
      }
      button.textContent = "处理中…";
      button.disabled = true;
      button.setAttribute("aria-busy", "true");
      return;
    }

    if (button.dataset.idleLabel) {
      button.textContent = button.dataset.idleLabel;
      delete button.dataset.idleLabel;
    }
    button.disabled = false;
    button.removeAttribute("aria-busy");
  }

  function openDialog(dialog) {
    if (dialog.open) {
      return;
    }
    if (typeof dialog.showModal === "function") {
      dialog.showModal();
    } else {
      dialog.setAttribute("open", "");
    }
  }

  function closeDialog(dialog) {
    if (!dialog.open) {
      return;
    }
    if (typeof dialog.close === "function") {
      dialog.close();
    } else {
      dialog.removeAttribute("open");
    }
  }

  function extractApiMessage(payload, status) {
    const direct = payload?.error?.error?.message;
    if (typeof direct === "string" && direct) {
      return direct;
    }
    const secondary = payload?.error?.message;
    if (typeof secondary === "string" && secondary) {
      return secondary;
    }
    const topLevel = payload?.message;
    if (typeof topLevel === "string" && topLevel) {
      return topLevel;
    }
    if (status === 401) {
      return "访问密钥无效或权限不足。";
    }
    if (status === 503) {
      return "管理员界面当前已禁用。";
    }
    return `请求失败（HTTP ${status}）。`;
  }

  async function apiRequest(path, options = {}) {
    if (!state.accessKey) {
      throw new ApiError("当前没有可用的访问密钥。", 401, { sessionExpired: true });
    }

    const headers = new Headers(options.headers || {});
    headers.set("Accept", "application/json");
    headers.set("Authorization", `Bearer ${state.accessKey}`);

    let requestBody = options.body;
    if (
      requestBody !== undefined &&
      requestBody !== null &&
      !(requestBody instanceof FormData) &&
      typeof requestBody !== "string"
    ) {
      headers.set("Content-Type", "application/json");
      requestBody = JSON.stringify(requestBody);
    }

    let response;
    try {
      response = await fetch(`${API_ROOT}${path}`, {
        method: options.method || "GET",
        headers,
        body: requestBody,
        signal: options.signal,
        cache: "no-store",
      });
    } catch {
      throw new ApiError("无法连接管理 API，请检查 kb-server 是否在线。");
    }

    const responseText = await response.text();
    let payload = null;
    if (responseText) {
      try {
        payload = JSON.parse(responseText);
      } catch {
        payload = null;
      }
    }

    if (!response.ok) {
      const sessionExpired = response.status === 401 && options.expireSession !== false;
      const error = new ApiError(extractApiMessage(payload, response.status), response.status, {
        sessionExpired,
      });
      if (sessionExpired) {
        endSession("访问密钥已失效，请重新验证。", "error");
      }
      throw error;
    }

    return payload;
  }

  async function handleSignIn(event) {
    event.preventDefault();
    elements.signInError.hidden = true;
    const candidate = elements.accessKeyInput.value;
    if (!candidate) {
      elements.signInError.textContent = "请输入管理员访问密钥。";
      elements.signInError.hidden = false;
      elements.accessKeyInput.focus();
      return;
    }

    elements.accessKeyInput.value = "";
    state.accessKey = candidate;
    elements.accessKeyInput.disabled = true;
    setButtonBusy(elements.signInSubmit, true);
    setSessionStatus("正在验证", "idle");

    try {
      const settings = await apiRequest("/settings", { expireSession: false });
      state.settings = normalizeSettings(settings);
      await enterAdminView();
    } catch (error) {
      state.accessKey = null;
      setSessionStatus("验证失败", "error");
      const message =
        error.status === 401
          ? "访问密钥无效或权限不足。"
          : error.status === 503
            ? "管理员界面当前已禁用。"
            : error.message;
      elements.signInError.textContent = message;
      elements.signInError.hidden = false;
      showFeedback(message, "error", true);
      elements.accessKeyInput.focus();
    } finally {
      elements.accessKeyInput.disabled = false;
      setButtonBusy(elements.signInSubmit, false);
    }
  }

  async function enterAdminView() {
    elements.authView.hidden = true;
    elements.adminView.hidden = false;
    elements.openSettingsButton.disabled = false;
    elements.logoutButton.disabled = false;
    elements.refreshCollectionsButton.disabled = false;
    elements.createCollectionSubmit.disabled = false;
    elements.collectionName.disabled = false;
    setSessionStatus("已连接", "connected");

    try {
      const collections = await fetchCollections();
      state.collections = collections;
      renderCollections(collections);
      if (collections.length > 0) {
        await selectCollection(collections[0].name);
      } else {
        clearCollectionSelection("尚未创建集合。先创建一个集合以开始管理文档。");
      }
      showFeedback("已连接管理 API。", "success");
    } catch (error) {
      if (error.sessionExpired) {
        throw error;
      }
      clearCollectionSelection("集合读取失败。请检查服务状态后重试。");
      reportError(elements.documentsStatus, error, "集合读取失败：");
    }
  }

  function endSession(message = "已退出管理界面。", tone = "info") {
    state.accessKey = null;
    state.collectionRequestToken += 1;
    state.documentRequestToken += 1;
    state.chunkRequestToken += 1;
    state.searchRequestToken += 1;
    if (state.confirmResolver) {
      state.confirmResolver(false);
    }
    state.confirmResolver = null;

    closeDialog(elements.settingsDialog);
    closeDialog(elements.documentDialog);
    closeDialog(elements.uploadDialog);
    closeDialog(elements.embeddingResultDialog);
    closeDialog(elements.confirmDialog);

    elements.adminView.hidden = true;
    elements.authView.hidden = false;
    elements.openSettingsButton.disabled = true;
    elements.logoutButton.disabled = true;
    setSessionStatus("等待验证", "idle");
    elements.signInError.hidden = true;
    elements.accessKeyInput.value = "";
    elements.accessKeyInput.disabled = false;
    setButtonBusy(elements.signInSubmit, false);

    elements.collectionList.replaceChildren();
    elements.documentsBody.replaceChildren();
    elements.searchResultsList.replaceChildren();
    elements.chunkList.replaceChildren();
    elements.collectionsCount.textContent = "0";
    elements.collectionsEmpty.hidden = false;
    elements.collectionsEmpty.textContent = "登录后读取集合。";
    elements.documentsEmpty.hidden = false;
    elements.documentsEmpty.textContent = "登录后读取文档。";
    elements.searchResultsSection.hidden = true;
    elements.searchSummary.textContent = "";
    elements.searchEmpty.hidden = true;
    elements.chunkCountLabel.textContent = "";
    elements.chunkEmpty.hidden = false;
    elements.chunkEmpty.textContent = "尚未选择文档。";
    elements.loadMoreChunksButton.hidden = true;
    elements.selectedDocumentSummary.hidden = true;
    elements.selectedCollectionName.textContent = "未选择集合";
    elements.selectedCollectionMeta.textContent = "从左侧集合索引中选择一个集合。";
    elements.selectedCollectionPoints.textContent = "—";
    elements.selectedCollectionVector.textContent = "—";
    elements.documentsPageLabel.textContent = "第 1 页";

    state.settings = null;
    state.collections = [];
    state.selectedCollection = null;
    state.documents = [];
    state.documentCursorStack = [null];
    state.documentCursorIndex = 0;
    state.documentNextCursor = null;
    state.selectedDocument = null;
    state.chunks = [];
    state.chunkNextCursor = null;
    state.chunkFocusIndex = null;
    state.searchSpec = null;
    state.settingsBusy = false;

    clearSettingsSensitiveControls();
    clearPrivateFormValues();
    disableWorkspaceControls();
    setStatus(elements.collectionFormStatus, "");
    setStatus(elements.documentsStatus, "");
    setStatus(elements.searchStatus, "");
    setStatus(elements.chunkStatus, "");
    showFeedback(message, tone);
    elements.accessKeyInput.focus();
  }

  function clearPrivateFormValues() {
    elements.createCollectionForm.reset();
    elements.documentForm.reset();
    elements.uploadForm.reset();
    elements.manualDocumentForm.reset();
    elements.searchForm.reset();
    elements.searchTopK.value = "10";
    setStatus(elements.documentFormStatus, "");
    setStatus(elements.uploadFormStatus, "");
    setStatus(elements.settingsFormStatus, "");
  }

  function disableWorkspaceControls() {
    const controls = [
      elements.refreshCollectionsButton,
      elements.createCollectionSubmit,
      elements.collectionName,
      elements.refreshDocumentsButton,
      elements.openDocumentDialogButton,
      elements.openUploadDialogButton,
      elements.deleteCollectionButton,
      elements.searchQuery,
      elements.searchTopK,
      elements.searchDocIds,
      elements.searchSubmit,
      elements.refreshSearchButton,
      elements.clearSearchButton,
      elements.manualDocId,
      elements.manualDocumentSubmit,
      elements.deleteSelectedDocumentButton,
      elements.documentsPreviousButton,
      elements.documentsNextButton,
      elements.loadMoreChunksButton,
    ];
    for (const control of controls) {
      control.disabled = true;
    }
  }

  async function fetchCollections() {
    const payload = await apiRequest("/collections");
    const source = Array.isArray(payload?.collections) ? payload.collections : [];
    return source
      .filter((item) => item && typeof item.name === "string")
      .map((item) => ({
        name: item.name,
        points_count: item.points_count,
        vector_size: item.vector_size,
      }));
  }

  function renderCollections(collections) {
    elements.collectionList.replaceChildren();
    elements.collectionsCount.textContent = String(collections.length);
    elements.collectionsEmpty.hidden = collections.length > 0;
    elements.collectionsEmpty.textContent = "尚未创建集合。";

    for (const collection of collections) {
      const item = document.createElement("li");
      const button = createTextElement("button", "collection-item");
      button.type = "button";
      button.setAttribute(
        "aria-current",
        state.selectedCollection?.name === collection.name ? "true" : "false",
      );

      const name = createTextElement("span", "collection-item-name", collection.name);
      const count = createTextElement(
        "span",
        "collection-item-count",
        `${valueOrDash(collection.points_count)} 点`,
      );
      const detail = createTextElement(
        "span",
        "collection-item-detail",
        `vector_size ${valueOrDash(collection.vector_size)}`,
      );
      button.append(name, count, detail);
      button.addEventListener("click", () => {
        void selectCollection(collection.name);
      });
      item.append(button);
      elements.collectionList.append(item);
    }
  }

  async function refreshCollections(options = {}) {
    const { chooseFirst = true, preserveNotice = false } = options;
    if (elements.refreshCollectionsButton.disabled && options.fromButton !== false) {
      return;
    }

    if (options.fromButton) {
      setButtonBusy(elements.refreshCollectionsButton, true);
    }

    try {
      const collections = await fetchCollections();
      state.collections = collections;
      renderCollections(collections);

      if (state.selectedCollection) {
        const refreshed = collections.find(
          (collection) => collection.name === state.selectedCollection.name,
        );
        if (refreshed) {
          state.selectedCollection = refreshed;
          updateSelectedCollectionHeader();
          renderCollections(collections);
        } else {
          clearCollectionSelection("当前集合已不可用，请重新选择。");
        }
      } else if (chooseFirst && collections.length > 0) {
        await selectCollection(collections[0].name);
      }

      if (!preserveNotice) {
        showFeedback("集合列表已刷新。", "success");
      }
      return collections;
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.collectionFormStatus, error, "集合刷新失败：");
      }
      return null;
    } finally {
      if (options.fromButton) {
        setButtonBusy(elements.refreshCollectionsButton, false);
        elements.refreshCollectionsButton.disabled = !state.accessKey;
      }
    }
  }

  function updateSelectedCollectionHeader() {
    const collection = state.selectedCollection;
    if (!collection) {
      elements.selectedCollectionName.textContent = "未选择集合";
      elements.selectedCollectionMeta.textContent = "从左侧集合索引中选择一个集合。";
      elements.selectedCollectionPoints.textContent = "—";
      elements.selectedCollectionVector.textContent = "—";
      return;
    }

    elements.selectedCollectionName.textContent = collection.name;
    elements.selectedCollectionMeta.textContent = "账册已连接当前集合；点数和维度来自集合列表。";
    elements.selectedCollectionPoints.textContent = valueOrDash(collection.points_count);
    elements.selectedCollectionVector.textContent = valueOrDash(collection.vector_size);
  }

  function setCollectionControlsEnabled(enabled) {
    elements.refreshDocumentsButton.disabled = !enabled;
    elements.openDocumentDialogButton.disabled = !enabled;
    elements.openUploadDialogButton.disabled = !enabled;
    elements.deleteCollectionButton.disabled = !enabled;
    elements.searchQuery.disabled = !enabled;
    elements.searchTopK.disabled = !enabled;
    elements.searchDocIds.disabled = !enabled;
    elements.searchSubmit.disabled = !enabled;
    elements.manualDocId.disabled = !enabled;
    elements.manualDocumentSubmit.disabled = !enabled;
  }

  function clearCollectionSelection(message) {
    state.selectedCollection = null;
    state.documents = [];
    state.documentCursorStack = [null];
    state.documentCursorIndex = 0;
    state.documentNextCursor = null;
    elements.documentsBody.replaceChildren();
    elements.documentsEmpty.hidden = false;
    elements.documentsEmpty.textContent = message;
    elements.documentsPageLabel.textContent = "第 1 页";
    elements.documentsPreviousButton.disabled = true;
    elements.documentsNextButton.disabled = true;
    updateSelectedCollectionHeader();
    renderCollections(state.collections);
    clearSearchResults();
    clearInspectorPanel();
    setCollectionControlsEnabled(false);
  }

  async function selectCollection(name) {
    const collection = state.collections.find((item) => item.name === name);
    if (!collection) {
      reportMessage(elements.documentsStatus, "无法定位所选集合，请刷新集合列表。", "error");
      return;
    }

    const requestToken = ++state.collectionRequestToken;
    state.selectedCollection = collection;
    state.selectedDocument = null;
    state.chunks = [];
    state.chunkNextCursor = null;
    state.chunkFocusIndex = null;
    state.documentCursorStack = [null];
    state.documentCursorIndex = 0;
    state.documentNextCursor = null;
    renderCollections(state.collections);
    updateSelectedCollectionHeader();
    setCollectionControlsEnabled(true);
    clearSearchResults();
    clearInspectorPanel();
    elements.manualDocId.value = "";

    const loaded = await loadDocumentPage({ resetPaging: true });
    if (requestToken !== state.collectionRequestToken) {
      return;
    }
    if (!loaded) {
      setStatus(elements.documentsStatus, "文档读取失败，可使用“刷新文档”重试。", "error");
    }
  }

  async function handleCreateCollection(event) {
    event.preventDefault();
    const name = elements.collectionName.value.trim();
    if (!name) {
      reportMessage(elements.collectionFormStatus, "请输入集合名称。", "error");
      elements.collectionName.focus();
      return;
    }

    setButtonBusy(elements.createCollectionSubmit, true);
    elements.collectionName.disabled = true;
    setStatus(elements.collectionFormStatus, "正在创建集合并等待服务端返回…");

    try {
      await apiRequest("/collections", {
        method: "POST",
        body: { name },
      });
      elements.collectionName.value = "";
      const collections = await fetchCollections();
      state.collections = collections;
      renderCollections(collections);
      const created = collections.find((collection) => collection.name === name);
      if (created) {
        await selectCollection(created.name);
      }
      setStatus(elements.collectionFormStatus, "集合已创建。", "success");
      showFeedback(`集合“${name}”已创建。`, "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.collectionFormStatus, error, "创建集合失败：");
      }
    } finally {
      setButtonBusy(elements.createCollectionSubmit, false);
      elements.collectionName.disabled = false;
    }
  }

  async function requestDeleteCollection() {
    const collection = state.selectedCollection;
    if (!collection) {
      return;
    }
    const accepted = await confirmDestructive(
      "删除集合",
      `将删除集合“${collection.name}”及其全部向量。此操作不可由管理界面恢复。`,
      "确认删除集合",
    );
    if (!accepted) {
      return;
    }

    setButtonBusy(elements.deleteCollectionButton, true);
    try {
      showFeedback(`正在删除集合“${collection.name}”…`);
      await apiRequest(`/collections/${encodeURIComponent(collection.name)}`, {
        method: "DELETE",
      });
      clearCollectionSelection("集合已删除。");
      const collections = await fetchCollections();
      state.collections = collections;
      renderCollections(collections);
      if (collections.length > 0) {
        await selectCollection(collections[0].name);
      }
      showFeedback(`集合“${collection.name}”已删除。`, "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.documentsStatus, error, "删除集合失败：");
      }
    } finally {
      setButtonBusy(elements.deleteCollectionButton, false);
      elements.deleteCollectionButton.disabled = !state.selectedCollection;
    }
  }

  function resetDocumentPaging() {
    state.documentCursorStack = [null];
    state.documentCursorIndex = 0;
    state.documentNextCursor = null;
  }

  async function loadDocumentPage(options = {}) {
    const collection = state.selectedCollection;
    if (!collection) {
      return false;
    }
    if (options.resetPaging) {
      resetDocumentPaging();
    }

    const requestToken = ++state.documentRequestToken;
    setStatus(elements.documentsStatus, "正在读取文档账册…");
    elements.documentsEmpty.hidden = false;
    elements.documentsEmpty.textContent = "正在读取文档…";
    elements.documentsBody.replaceChildren();
    elements.documentsPreviousButton.disabled = true;
    elements.documentsNextButton.disabled = true;
    setButtonBusy(elements.refreshDocumentsButton, true);

    const query = new URLSearchParams({ limit: String(DOCUMENT_PAGE_SIZE) });
    const cursor = state.documentCursorStack[state.documentCursorIndex];
    if (cursor) {
      query.set("cursor", cursor);
    }

    try {
      const payload = await apiRequest(
        `/collections/${encodeURIComponent(collection.name)}/documents?${query.toString()}`,
      );
      if (
        requestToken !== state.documentRequestToken ||
        state.selectedCollection?.name !== collection.name
      ) {
        return false;
      }

      const items = Array.isArray(payload?.items) ? payload.items : [];
      state.documents = items.filter((item) => item && typeof item.doc_id === "string");
      state.documentNextCursor =
        typeof payload?.next_cursor === "string" && payload.next_cursor
          ? payload.next_cursor
          : null;
      renderDocuments(state.documents);
      updateDocumentPagination();
      setStatus(
        elements.documentsStatus,
        state.documents.length > 0 ? `已载入 ${state.documents.length} 个文档。` : "当前页没有文档。",
        "success",
      );
      return true;
    } catch (error) {
      if (requestToken === state.documentRequestToken && !error.sessionExpired) {
        elements.documentsBody.replaceChildren();
        elements.documentsEmpty.hidden = false;
        elements.documentsEmpty.textContent = "文档读取失败。";
        reportError(elements.documentsStatus, error, "文档读取失败：");
      }
      return false;
    } finally {
      if (requestToken === state.documentRequestToken) {
        setButtonBusy(elements.refreshDocumentsButton, false);
        elements.refreshDocumentsButton.disabled = !state.selectedCollection;
        updateDocumentPagination();
      }
    }
  }

  function renderDocuments(documents) {
    elements.documentsBody.replaceChildren();
    elements.documentsEmpty.hidden = documents.length > 0;
    if (documents.length === 0) {
      elements.documentsEmpty.textContent = "当前集合还没有文档。可录入文本或上传文件。";
      return;
    }

    for (const documentRecord of documents) {
      const row = document.createElement("tr");
      row.dataset.docId = documentRecord.doc_id;
      row.classList.toggle(
        "is-selected",
        state.selectedDocument?.doc_id === documentRecord.doc_id,
      );

      const documentCell = document.createElement("td");
      documentCell.append(createTextElement("span", "mobile-label", "文档 / 来源"));
      const documentBody = document.createElement("div");
      const head = createTextElement("div", "generated-document-head");
      const title = createTextElement(
        "h4",
        "document-title",
        documentRecord.title || documentRecord.source || documentRecord.doc_id,
      );
      head.append(title);
      documentBody.append(head);
      documentBody.append(
        createTextElement("span", "document-id", documentRecord.doc_id),
      );
      if (documentRecord.source) {
        documentBody.append(
          createTextElement("span", "document-source", `来源：${documentRecord.source}`),
        );
      }
      if (Array.isArray(documentRecord.tags) && documentRecord.tags.length > 0) {
        const tagRow = createTextElement("div", "tag-row");
        for (const tag of documentRecord.tags) {
          tagRow.append(createTextElement("span", "tag", valueOrDash(tag)));
        }
        documentBody.append(tagRow);
      }
      documentCell.append(documentBody);

      const chunkCell = document.createElement("td");
      chunkCell.append(createTextElement("span", "mobile-label", "片段"));
      chunkCell.append(
        createTextElement(
          "span",
          "chunk-count",
          `${valueOrDash(documentRecord.chunk_count)} 段`,
        ),
      );

      const modelCell = document.createElement("td");
      modelCell.append(createTextElement("span", "mobile-label", "向量模型"));
      modelCell.append(
        createTextElement(
          "span",
          "model-value",
          valueOrDash(documentRecord.embedding_model),
        ),
      );

      const dateCell = document.createElement("td");
      dateCell.append(createTextElement("span", "mobile-label", "创建时间"));
      dateCell.append(
        createTextElement("span", "date-value", formatDate(documentRecord.created_at)),
      );

      const actionsCell = document.createElement("td");
      actionsCell.append(createTextElement("span", "mobile-label", "操作"));
      const actions = createTextElement("div", "row-actions");
      const inspectButton = createTextElement("button", "text-button", "查看片段");
      inspectButton.type = "button";
      inspectButton.addEventListener("click", () => {
        void selectDocument(documentRecord, { scroll: true });
      });
      const deleteButton = createTextElement("button", "text-button danger-text", "删除");
      deleteButton.type = "button";
      deleteButton.addEventListener("click", () => {
        void deleteDocument(documentRecord.doc_id);
      });
      actions.append(inspectButton, deleteButton);
      actionsCell.append(actions);

      row.append(documentCell, chunkCell, modelCell, dateCell, actionsCell);
      elements.documentsBody.append(row);
    }
  }

  function updateDocumentPagination() {
    elements.documentsPageLabel.textContent = `第 ${state.documentCursorIndex + 1} 页`;
    elements.documentsPreviousButton.disabled = state.documentCursorIndex === 0;
    elements.documentsNextButton.disabled = !state.documentNextCursor;
  }

  async function handlePreviousDocumentPage() {
    if (state.documentCursorIndex === 0) {
      return;
    }
    state.documentCursorIndex -= 1;
    await loadDocumentPage();
  }

  async function handleNextDocumentPage() {
    if (!state.documentNextCursor) {
      return;
    }
    const nextIndex = state.documentCursorIndex + 1;
    if (!state.documentCursorStack[nextIndex]) {
      state.documentCursorStack[nextIndex] = state.documentNextCursor;
    }
    state.documentCursorIndex = nextIndex;
    await loadDocumentPage();
  }

  async function selectDocument(documentRecord, options = {}) {
    if (!state.selectedCollection) {
      return;
    }
    state.selectedDocument = documentRecord;
    state.chunkFocusIndex =
      options.focusChunkIndex === null || options.focusChunkIndex === undefined
        ? null
        : String(options.focusChunkIndex);

    for (const row of elements.documentsBody.querySelectorAll("tr[data-doc-id]")) {
      row.classList.toggle("is-selected", row.dataset.docId === documentRecord.doc_id);
    }

    elements.manualDocId.value = documentRecord.doc_id;
    elements.selectedDocumentSummary.hidden = false;
    elements.selectedDocumentTitle.textContent =
      documentRecord.title || documentRecord.source || documentRecord.doc_id;
    const metaParts = [`doc_id ${documentRecord.doc_id}`];
    if (documentRecord.source) {
      metaParts.push(`source ${documentRecord.source}`);
    }
    if (documentRecord.chunk_count !== null && documentRecord.chunk_count !== undefined) {
      metaParts.push(`${documentRecord.chunk_count} 段`);
    }
    elements.selectedDocumentMeta.textContent = metaParts.join(" · ");
    elements.deleteSelectedDocumentButton.disabled = false;

    await loadChunks({ reset: true });
    if (options.scroll) {
      const behavior = window.matchMedia("(prefers-reduced-motion: reduce)").matches
        ? "auto"
        : "smooth";
      elements.inspector.scrollIntoView({ block: "start", behavior });
    }
  }

  async function handleManualDocumentInspect(event) {
    event.preventDefault();
    if (!state.selectedCollection) {
      return;
    }
    const docId = elements.manualDocId.value.trim();
    if (!docId) {
      reportMessage(elements.chunkStatus, "请输入完整文档 ID。", "error");
      elements.manualDocId.focus();
      return;
    }
    const known = state.documents.find((item) => item.doc_id === docId);
    await selectDocument(known || { doc_id: docId }, { scroll: true });
  }

  async function deleteDocument(docId) {
    if (!state.selectedCollection || !docId) {
      return;
    }
    const record = state.documents.find((item) => item.doc_id === docId);
    const label = record?.title || record?.source || docId;
    const accepted = await confirmDestructive(
      "删除文档",
      `将删除文档“${label}”及其全部分片。此操作不可由管理界面恢复。`,
      "确认删除文档",
    );
    if (!accepted) {
      return;
    }

    setButtonBusy(elements.deleteSelectedDocumentButton, true);
    setDocumentRowActionsDisabled(true);
    try {
      showFeedback("正在删除文档…");
      const payload = await apiRequest(
        `/collections/${encodeURIComponent(state.selectedCollection.name)}/documents?${new URLSearchParams(
          { doc_id: docId },
        ).toString()}`,
        { method: "DELETE" },
      );
      if (state.selectedDocument?.doc_id === docId) {
        clearInspectorPanel();
      }
      clearSearchResults();
      resetDocumentPaging();
      await loadDocumentPage({ resetPaging: true });
      await refreshCollections({ chooseFirst: false, preserveNotice: true, fromButton: false });
      const deletedPoints = valueOrDash(payload?.deleted_points);
      showFeedback(`文档已删除，移除 ${deletedPoints} 个片段。`, "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.documentsStatus, error, "删除文档失败：");
      }
    } finally {
      setButtonBusy(elements.deleteSelectedDocumentButton, false);
      elements.deleteSelectedDocumentButton.disabled = !state.selectedDocument;
      setDocumentRowActionsDisabled(false);
    }
  }

  function setDocumentRowActionsDisabled(disabled) {
    for (const button of elements.documentsBody.querySelectorAll("button")) {
      button.disabled = disabled;
    }
  }

  function clearInspectorPanel() {
    state.chunkRequestToken += 1;
    state.selectedDocument = null;
    state.chunks = [];
    state.chunkNextCursor = null;
    state.chunkFocusIndex = null;
    elements.selectedDocumentSummary.hidden = true;
    elements.selectedDocumentTitle.textContent = "";
    elements.selectedDocumentMeta.textContent = "";
    elements.chunkList.replaceChildren();
    elements.chunkEmpty.hidden = false;
    elements.chunkEmpty.textContent = "尚未选择文档。";
    elements.chunkCountLabel.textContent = "";
    elements.loadMoreChunksButton.hidden = true;
    elements.loadMoreChunksButton.disabled = true;
    elements.deleteSelectedDocumentButton.disabled = true;
    elements.manualDocId.value = "";
    setStatus(elements.chunkStatus, "");
    for (const row of elements.documentsBody.querySelectorAll("tr.is-selected")) {
      row.classList.remove("is-selected");
    }
  }

  async function loadChunks(options = {}) {
    const collection = state.selectedCollection;
    const selectedDocument = state.selectedDocument;
    if (!collection || !selectedDocument) {
      return false;
    }

    if (options.reset) {
      state.chunks = [];
      state.chunkNextCursor = null;
      elements.chunkList.replaceChildren();
      elements.chunkCountLabel.textContent = "";
    }

    const requestToken = ++state.chunkRequestToken;
    const cursor = options.reset ? null : state.chunkNextCursor;
    const query = new URLSearchParams({
      doc_id: selectedDocument.doc_id,
      limit: String(CHUNK_PAGE_SIZE),
    });
    if (cursor) {
      query.set("cursor", cursor);
    }

    setStatus(elements.chunkStatus, "正在读取片段账册…");
    elements.chunkEmpty.hidden = false;
    elements.chunkEmpty.textContent = "正在读取片段…";
    elements.loadMoreChunksButton.hidden = true;
    elements.loadMoreChunksButton.disabled = true;

    try {
      const payload = await apiRequest(
        `/collections/${encodeURIComponent(collection.name)}/documents/chunks?${query.toString()}`,
      );
      if (
        requestToken !== state.chunkRequestToken ||
        state.selectedDocument?.doc_id !== selectedDocument.doc_id ||
        state.selectedCollection?.name !== collection.name
      ) {
        return false;
      }

      const items = Array.isArray(payload?.items) ? payload.items : [];
      state.chunks.push(...items);
      state.chunkNextCursor =
        typeof payload?.next_cursor === "string" && payload.next_cursor
          ? payload.next_cursor
          : null;

      for (const chunk of items) {
        elements.chunkList.append(renderChunk(chunk));
      }
      elements.chunkCountLabel.textContent = `已载入 ${state.chunks.length} 段`;
      elements.chunkEmpty.hidden = state.chunks.length > 0;
      if (state.chunks.length === 0) {
        elements.chunkEmpty.textContent = "服务端未返回片段。";
      }
      elements.loadMoreChunksButton.hidden = !state.chunkNextCursor;
      elements.loadMoreChunksButton.disabled = !state.chunkNextCursor;

      const target = focusChunkTarget(state.chunkFocusIndex);
      if (target) {
        target.scrollIntoView({
          block: "center",
          behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches
            ? "auto"
            : "smooth",
        });
      }

      if (state.chunkFocusIndex && !target) {
        setStatus(
          elements.chunkStatus,
          state.chunkNextCursor
            ? "目标片段尚未出现在已加载结果中，请继续加载。"
            : "目标片段不在当前返回的片段中。",
          "warning",
        );
      } else if (state.chunkNextCursor) {
        setStatus(
          elements.chunkStatus,
          `已载入 ${state.chunks.length} 段，仍有更多片段。`,
          "success",
        );
      } else {
        setStatus(elements.chunkStatus, `已载入 ${state.chunks.length} 段。`, "success");
      }
      return true;
    } catch (error) {
      if (requestToken === state.chunkRequestToken && !error.sessionExpired) {
        elements.chunkEmpty.hidden = state.chunks.length > 0;
        if (state.chunks.length === 0) {
          elements.chunkEmpty.textContent = "片段读取失败。";
        }
        reportError(elements.chunkStatus, error, "片段读取失败：");
      }
      return false;
    }
  }

  function renderChunk(chunk) {
    const item = document.createElement("li");
    item.className = "chunk-item";
    if (
      state.chunkFocusIndex !== null &&
      state.chunkFocusIndex !== undefined &&
      String(chunk.chunk_index) === String(state.chunkFocusIndex)
    ) {
      item.classList.add("is-target");
      item.dataset.targetChunk = "true";
    }

    const topLine = createTextElement("div", "chunk-topline");
    topLine.append(
      createTextElement("span", "chunk-number", `#${valueOrDash(chunk.chunk_index)}`),
      createTextElement("p", "chunk-meta", `point_id ${valueOrDash(chunk.point_id)}`),
    );

    const text = createTextElement("p", "chunk-text", valueOrDash(chunk.text));
    const data = document.createElement("details");
    data.className = "chunk-data";
    data.append(createTextElement("summary", "", "片段数据"));

    const dataGrid = createTextElement("dl", "data-grid");
    appendDataPair(dataGrid, "point_id", valueOrDash(chunk.point_id));
    appendDataPair(dataGrid, "chunk_index", valueOrDash(chunk.chunk_index));
    appendDataPair(dataGrid, "heading_path", formatHeadingPath(chunk.heading_path));
    data.append(dataGrid);

    if (chunk.extra !== null && chunk.extra !== undefined) {
      const extraLabel = createTextElement("p", "field-help", "extra");
      const extraPre = createTextElement("pre", "data-pre", formatJson(chunk.extra));
      data.append(extraLabel, extraPre);
    }

    item.append(topLine, text, data);
    return item;
  }

  function appendDataPair(list, label, value) {
    const wrapper = document.createElement("div");
    wrapper.append(
      createTextElement("dt", "", label),
      createTextElement("dd", "", value),
    );
    list.append(wrapper);
  }

  function focusChunkTarget(index) {
    if (index === null || index === undefined) {
      return null;
    }
    return elements.chunkList.querySelector("[data-target-chunk='true']");
  }

  function collectSearchSpec() {
    const query = elements.searchQuery.value.trim();
    if (!query) {
      throw new Error("请输入查询文本。");
    }
    const topK = elements.searchTopK.valueAsNumber;
    if (!Number.isInteger(topK) || topK < 1) {
      throw new Error("返回条数必须是大于 0 的整数。");
    }
    const docIds = parseIdList(elements.searchDocIds.value);
    return {
      query,
      topK,
      docIds,
    };
  }

  async function handleSearchSubmit(event) {
    event.preventDefault();
    if (!state.selectedCollection) {
      return;
    }
    let spec;
    try {
      spec = collectSearchSpec();
    } catch (error) {
      reportMessage(elements.searchStatus, error.message, "error");
      return;
    }
    state.searchSpec = spec;
    await runSearch(spec);
  }

  async function runSearch(spec) {
    const collection = state.selectedCollection;
    if (!collection) {
      return;
    }

    const requestToken = ++state.searchRequestToken;
    elements.searchResultsSection.hidden = false;
    elements.searchResultsList.replaceChildren();
    elements.searchEmpty.hidden = true;
    elements.searchSummary.textContent = "";
    setStatus(elements.searchStatus, "正在运行检索…");
    setButtonBusy(elements.searchSubmit, true);
    elements.refreshSearchButton.disabled = true;
    elements.clearSearchButton.disabled = false;

    const body = {
      query: spec.query,
      top_k: spec.topK,
    };
    if (spec.docIds.length > 0) {
      body.filter = { doc_ids: spec.docIds };
    }

    try {
      const payload = await apiRequest(
        `/collections/${encodeURIComponent(collection.name)}/search`,
        {
          method: "POST",
          body,
        },
      );
      if (
        requestToken !== state.searchRequestToken ||
        state.selectedCollection?.name !== collection.name
      ) {
        return;
      }

      const results = Array.isArray(payload?.results) ? payload.results : [];
      for (const result of results) {
        elements.searchResultsList.append(renderSearchResult(result));
      }
      elements.searchEmpty.hidden = results.length > 0;
      const filterLabel =
        spec.docIds.length > 0 ? ` · 限定 ${spec.docIds.length} 个文档` : "";
      elements.searchSummary.textContent = `${results.length} 条命中 · top_k ${spec.topK}${filterLabel}`;
      setStatus(elements.searchStatus, `检索完成，共 ${results.length} 条命中。`, "success");
    } catch (error) {
      if (requestToken === state.searchRequestToken && !error.sessionExpired) {
        elements.searchEmpty.hidden = false;
        elements.searchEmpty.textContent = "检索失败，请检查条件后重试。";
        reportError(elements.searchStatus, error, "检索失败：");
      }
    } finally {
      if (requestToken === state.searchRequestToken) {
        setButtonBusy(elements.searchSubmit, false);
        elements.searchSubmit.disabled = !state.selectedCollection;
        elements.refreshSearchButton.disabled = false;
        elements.clearSearchButton.disabled = false;
      }
    }
  }

  function renderSearchResult(result) {
    const item = document.createElement("li");
    item.className = "search-hit";

    const head = createTextElement("div", "search-hit-head");
    head.append(
      createTextElement(
        "span",
        "search-hit-location",
        `doc_id ${valueOrDash(result.doc_id)} · chunk_index ${valueOrDash(result.chunk_index)}`,
      ),
    );

    const scores = createTextElement("div", "score-pair");
    scores.append(
      createTextElement("span", "score-item", `score ${valueOrDash(result.score)}`),
    );
    if (result.rerank_score !== null && result.rerank_score !== undefined) {
      scores.append(
        createTextElement(
          "span",
          "score-item",
          `rerank_score ${valueOrDash(result.rerank_score)}`,
        ),
      );
    }
    head.append(scores);

    const text = createTextElement("p", "search-hit-text", valueOrDash(result.text));
    const actions = createTextElement("div", "search-hit-actions");
    const inspectButton = createTextElement("button", "button button-quiet", "在片段账册中打开");
    inspectButton.type = "button";
    inspectButton.addEventListener("click", () => {
      const metadata =
        result.metadata && typeof result.metadata === "object" ? result.metadata : {};
      const known = state.documents.find((entry) => entry.doc_id === result.doc_id);
      const documentRecord = known || {
        doc_id: valueOrDash(result.doc_id),
        title: typeof metadata.title === "string" ? metadata.title : "",
        source: typeof metadata.source === "string" ? metadata.source : "",
        tags: Array.isArray(metadata.tags) ? metadata.tags : [],
      };
      void selectDocument(documentRecord, {
        focusChunkIndex: result.chunk_index,
        scroll: true,
      });
    });
    actions.append(inspectButton);

    item.append(head, text, actions);
    return item;
  }

  function clearSearchResults() {
    state.searchRequestToken += 1;
    state.searchSpec = null;
    elements.searchResultsSection.hidden = true;
    elements.searchResultsList.replaceChildren();
    elements.searchSummary.textContent = "";
    elements.searchEmpty.hidden = true;
    elements.refreshSearchButton.disabled = true;
    elements.clearSearchButton.disabled = true;
    elements.searchQuery.value = "";
    elements.searchDocIds.value = "";
    setStatus(elements.searchStatus, "");
  }

  async function handleRefreshSearch() {
    if (!state.searchSpec) {
      return;
    }
    await runSearch(state.searchSpec);
  }

  function openDocumentDialog() {
    if (!state.selectedCollection) {
      return;
    }
    elements.documentForm.reset();
    setStatus(elements.documentFormStatus, "");
    openDialog(elements.documentDialog);
    elements.documentDocId.focus();
  }

  async function handleDocumentSubmit(event) {
    event.preventDefault();
    if (!state.selectedCollection) {
      return;
    }
    const text = elements.documentText.value;
    if (!text.trim()) {
      reportMessage(elements.documentFormStatus, "正文不能为空。", "error");
      elements.documentText.focus();
      return;
    }

    const metadata = {};
    const title = elements.documentTitle.value.trim();
    const source = elements.documentSource.value.trim();
    const tags = parseCommaList(elements.documentTags.value);
    if (title) {
      metadata.title = title;
    }
    if (source) {
      metadata.source = source;
    }
    if (tags.length > 0) {
      metadata.tags = tags;
    }

    const body = { text, metadata };
    const docId = elements.documentDocId.value.trim();
    const format = elements.documentFormat.value;
    if (docId) {
      body.doc_id = docId;
    }
    if (format) {
      body.format = format;
    }

    setButtonBusy(elements.documentSubmit, true);
    setStatus(elements.documentFormStatus, "正在提交文档并生成向量…");

    try {
      await apiRequest(
        `/collections/${encodeURIComponent(state.selectedCollection.name)}/documents`,
        {
          method: "POST",
          body,
        },
      );
      closeDialog(elements.documentDialog);
      await refreshDocumentsAfterMutation();
      showFeedback("文档已录入并完成切片。", "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.documentFormStatus, error, "文档录入失败：");
      }
    } finally {
      setButtonBusy(elements.documentSubmit, false);
    }
  }

  function openUploadDialog() {
    if (!state.selectedCollection) {
      return;
    }
    elements.uploadForm.reset();
    setStatus(elements.uploadFormStatus, "");
    openDialog(elements.uploadDialog);
    elements.uploadFile.focus();
  }

  async function handleUploadSubmit(event) {
    event.preventDefault();
    if (!state.selectedCollection) {
      return;
    }
    const file = elements.uploadFile.files?.[0];
    if (!file) {
      reportMessage(elements.uploadFormStatus, "请选择要上传的文件。", "error");
      elements.uploadFile.focus();
      return;
    }

    const body = new FormData();
    body.append("file", file, file.name);
    appendOptionalFormValue(body, "doc_id", elements.uploadDocId.value);
    appendOptionalFormValue(body, "title", elements.uploadTitle.value);
    appendOptionalFormValue(body, "source", elements.uploadSource.value);
    appendOptionalFormValue(body, "tags", elements.uploadTags.value);
    appendOptionalFormValue(body, "parser", elements.uploadParser.value);

    setButtonBusy(elements.uploadSubmit, true);
    elements.uploadForm.setAttribute("aria-busy", "true");
    setStatus(
      elements.uploadFormStatus,
      "正在上传文件；解析和向量化结束后才会返回结果…",
    );

    try {
      await apiRequest(
        `/collections/${encodeURIComponent(state.selectedCollection.name)}/documents/upload`,
        {
          method: "POST",
          body,
        },
      );
      closeDialog(elements.uploadDialog);
      await refreshDocumentsAfterMutation();
      showFeedback("文件已上传并完成入库。", "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.uploadFormStatus, error, "文件上传失败：");
      }
    } finally {
      elements.uploadForm.removeAttribute("aria-busy");
      setButtonBusy(elements.uploadSubmit, false);
    }
  }

  function appendOptionalFormValue(formData, name, rawValue) {
    const value = rawValue.trim();
    if (value) {
      formData.append(name, name === "tags" ? parseCommaList(value).join(", ") : value);
    }
  }

  async function refreshDocumentsAfterMutation() {
    clearSearchResults();
    resetDocumentPaging();
    await loadDocumentPage({ resetPaging: true });
    await refreshCollections({
      chooseFirst: false,
      preserveNotice: true,
      fromButton: false,
    });
  }

  function normalizeSettings(settings) {
    return settings && typeof settings === "object" ? settings : {};
  }

  function embeddingProviderLabel(provider) {
    return {
      local: "本地模型",
      api: "API 服务",
      mock: "Mock 演示",
    }[provider] || "未知提供方";
  }

  function updateEmbeddingProviderUi() {
    const provider = settingsControls.embedding_provider.value;
    elements.settingEmbeddingLocalFields.hidden = provider !== "local";
    elements.settingEmbeddingApiFields.hidden = provider !== "api";
    elements.settingEmbeddingMockFields.hidden = provider !== "mock";
    elements.settingEmbeddingProviderHelp.textContent =
      {
        local: "本地模型在当前进程内运行，无需 API 密钥；首次使用可能需要下载模型。",
        api: "保存时会验证 API 地址、模型与协议；测试操作只使用最近一次保存的配置。",
        mock: "Mock 返回确定性向量，不访问任何外部服务，适合离线验证。",
      }[provider] || "请选择嵌入提供方。";
    updateEmbeddingProtocolHint();
    updateLocalDimensionHelp();
  }

  function updateLocalDimensionHelp() {
    const model = settingsControls.embedding_local_model.value;
    const dimension = state.settings?.embedding_local_model_dimensions?.[model];
    const input = settingsControls.embedding_local_dimensions;
    if (Number.isSafeInteger(dimension) && dimension > 0) {
      input.max = String(dimension);
      elements.settingEmbeddingLocalDimensionsHelp.textContent =
        `填 0 使用模型原始输出（${dimension} 维），或手填 1–${dimension}。`;
    } else {
      input.removeAttribute("max");
      elements.settingEmbeddingLocalDimensionsHelp.textContent =
        "填 0 使用模型原始维度，自定义值不能超过原始维度。";
    }
  }

  function updateEmbeddingProtocolHint() {
    const protocol = settingsControls.embedding_api_protocol.value;
    elements.settingEmbeddingApiProtocolHint.textContent =
      protocol === "dashscope"
        ? "自动调用 DashScope 原生 TextEmbedding 接口，查询与文档使用不同输入类型。"
        : "自动追加 /embeddings，适用于 OpenAI、兼容模式、SiliconFlow、Ollama 与兼容网关。";
  }

  function activeEmbeddingModel(settings) {
    if (settings.embedding_model !== null && settings.embedding_model !== undefined) {
      return String(settings.embedding_model);
    }
    if (settings.embedding_provider === "local") {
      return settings.embedding_local_model || "—";
    }
    if (settings.embedding_provider === "api") {
      return settings.embedding_api_model || "—";
    }
    return settings.embedding_provider === "mock" ? "mock" : "—";
  }

  function fillEmbeddingReadout(settings) {
    const provider = settings.embedding_provider || "";
    elements.settingEmbeddingProviderSummary.textContent = embeddingProviderLabel(provider);
    elements.settingEmbeddingProviderSummary.dataset.provider = provider;
    elements.settingEmbeddingModel.textContent = valueOrDash(activeEmbeddingModel(settings));

    const dimension = Number(settings.embedding_dimension);
    elements.settingEmbeddingDimension.textContent =
      Number.isInteger(dimension) && dimension >= 0 ? `${dimension} 维` : "—";
  }

  function rerankSummary(settings) {
    const mode = String(settings.rerank_provider || "").trim().toLowerCase();
    if (mode === "local") {
      return `本地模型 · ${valueOrDash(settings.rerank_local_model)}`;
    }
    if (mode === "api") {
      return `API 服务 · ${valueOrDash(settings.rerank_api_model)}`;
    }
    return mode === "disabled" ? "关闭重排" : "—";
  }

  function parsingSummary(settings) {
    if (settings.parsing_provider === "mineru_cloud") {
      return `MinerU 云端 API · ${valueOrDash(settings.mineru_model_version)}`;
    }
    return { native: "原生解析", mineru_local: "本地 MinerU" }[settings.parsing_provider] || "—";
  }

  function updateProviderPanels() {
    const rerank = readSetting("rerank_provider");
    elements.rerankDisabledFields.hidden = rerank !== "disabled";
    elements.rerankEnabledFields.hidden = rerank === "disabled";
    elements.rerankLocalFields.hidden = rerank !== "local";
    elements.rerankApiFields.hidden = rerank !== "api";
    elements.rerankRouteStep.textContent = {
      disabled: "按相似度排序",
      local: "本地模型重排",
      api: "API 重排",
    }[rerank];

    const parsing = readSetting("parsing_provider");
    const model = readSetting("mineru_model_version");
    elements.parsingNativeFields.hidden = parsing !== "native";
    elements.parsingLocalFields.hidden = parsing !== "mineru_local";
    elements.parsingCloudFields.hidden = parsing !== "mineru_cloud";
    elements.parsingKeepLocalNote.hidden = parsing === "native";
    elements.clearInactiveMineruKey.hidden = parsing === "mineru_cloud" || !state.settings?.mineru_api_key_set;
    if (parsing === "mineru_cloud") {
      const token = secretControls.find((entry) => entry.field === "mineru_api_key");
      token.clear.checked = false;
      token.input.disabled = false;
    }
    elements.parsingRouteInput.textContent = parsing === "native"
      ? "文本类上传"
      : parsing === "mineru_cloud" && model === "MinerU-HTML" ? "指定云解析的 HTML" : "PDF 等文档";
    elements.parsingRouteStep.textContent = {
      native: "服务内解析",
      mineru_local: "自部署 MinerU",
      mineru_cloud: "MinerU 云端",
    }[parsing];
  }

  function selectSettingsTab(section, focus = false) {
    state.settingsTab = section;
    for (const tab of settingsTabs) {
      const selected = tab.dataset.settingsTab === section;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
      byId(tab.getAttribute("aria-controls")).hidden = !selected;
      if (selected && focus) {
        tab.focus();
      }
    }
    elements.settingsFields.closest(".settings-body").scrollTop = 0;
    setStatus(elements.settingsFormStatus, "");
    syncSettingsActions();
  }

  async function openSettings() {
    if (!state.accessKey) {
      return;
    }
    setButtonBusy(elements.openSettingsButton, true);
    try {
      const settings = await apiRequest("/settings");
      state.settings = normalizeSettings(settings);
      fillSettingsForm(state.settings);
      selectSettingsTab(state.settingsTab);
      openDialog(elements.settingsDialog);
    } catch (error) {
      if (!error.sessionExpired) {
        showFeedback(`设置读取失败：${error.message}`, "error", true);
      }
    } finally {
      setButtonBusy(elements.openSettingsButton, false);
      elements.openSettingsButton.disabled = !state.accessKey;
    }
  }

  function allSectionFields(section) {
    const { provider, modes } = SETTINGS_SECTIONS[section];
    return [...new Set([provider, ...Object.values(modes).flat()])];
  }

  function readSetting(field) {
    if (settingsRadios[field]) {
      return settingsRadios[field].find((radio) => radio.checked)?.value || "";
    }
    return settingsControls[field].value.trim();
  }

  function savedSetting(field, settings = state.settings) {
    const value = String(settings?.[field] ?? (field === "embedding_local_dimensions" ? 0 : "")).trim();
    return ["rerank_provider", "embedding_local_model"].includes(field) ? value.toLowerCase() : value;
  }

  function fillSettingsForm(settings, selectedSection = null, savedFields = null) {
    const sections = selectedSection ? [selectedSection] : Object.keys(SETTINGS_SECTIONS);
    for (const section of sections) {
      for (const field of allSectionFields(section)) {
        if (savedFields && !savedFields.includes(field)) {
          continue;
        }
        const value = savedSetting(field, settings);
        if (settingsRadios[field]) {
          const radios = settingsRadios[field];
          (radios.find((radio) => radio.value === value) || radios[0]).checked = true;
        } else if (settingsControls[field]) {
          const control = settingsControls[field];
          if (field === "rerank_local_model") {
            control.querySelector("[data-saved-model]")?.remove();
            if (value && ![...control.options].some((option) => option.value === value)) {
              const option = new Option(value, value);
              option.dataset.savedModel = "true";
              control.add(option);
            }
          }
          control.value = value;
        } else {
          const secret = secretControls.find((entry) => entry.field === field);
          secret.input.value = "";
          secret.input.disabled = false;
          secret.clear.checked = false;
          const isSet = Boolean(settings[secret.statusField]);
          secret.status.textContent = isSet ? "已配置" : "未配置";
          secret.status.dataset.set = String(isSet);
          secret.clear.disabled = !isSet;
        }
      }
    }
    fillEmbeddingReadout(settings);
    elements.rerankActiveSummary.textContent = rerankSummary(settings);
    elements.parsingActiveSummary.textContent = parsingSummary(settings);
    updateEmbeddingProviderUi();
    updateProviderPanels();
    syncSettingsActions();
  }

  function clearSettingsSensitiveControls() {
    for (const secret of secretControls) {
      secret.input.value = "";
      secret.input.disabled = false;
      secret.clear.checked = false;
      secret.clear.disabled = true;
      secret.status.textContent = "未配置";
      secret.status.dataset.set = "false";
    }
    elements.settingsForm.reset();
    elements.settingEmbeddingProviderSummary.textContent = "未加载";
    delete elements.settingEmbeddingProviderSummary.dataset.provider;
    elements.settingEmbeddingModel.textContent = "—";
    elements.settingEmbeddingDimension.textContent = "—";
    elements.embeddingProbeChart.hidden = true;
    elements.embeddingProbeBars.replaceChildren();
    elements.embeddingProbeSampleMeta.textContent = "—";
    state.settingsProbeAbort?.abort();
    updateEmbeddingProviderUi();
    updateProviderPanels();
    setSettingsBusy(false);
  }

  function handleSecretClearToggle(secret) {
    if (secret.clear.checked) {
      secret.input.value = "";
      secret.input.disabled = true;
    } else {
      secret.input.disabled = false;
    }
  }

  function readIntegerSetting(field, control) {
    const config = INTEGER_SETTINGS[field];
    const max = control.max === "" ? config.max : Number(control.max);
    const numericValue = Number(control.value);
    if (
      control.value.trim() === "" ||
      !Number.isSafeInteger(numericValue) ||
      numericValue < config.min ||
      (max !== null && numericValue > max)
    ) {
      const range =
        max === null
          ? `大于等于 ${config.min}`
          : `${config.min} 到 ${max}`;
      control.focus();
      throw new Error(`${config.label}必须是 ${range} 的整数。`);
    }
    return numericValue;
  }

  function activeSectionFields(section) {
    const { provider, modes } = SETTINGS_SECTIONS[section];
    const mode = readSetting(provider);
    const fields = [provider, ...(modes[mode] || [])];
    // A cloud token may be removed as part of switching away from cloud.
    if (section === "parsing" && mode !== "mineru_cloud"
        && !elements.clearInactiveMineruKey.hidden
        && secretControls.find((entry) => entry.field === "mineru_api_key").clear.checked) {
      fields.push("mineru_api_key");
    }
    return fields;
  }

  function settingHasChange(field) {
    const secret = secretControls.find((entry) => entry.field === field);
    return secret
      ? secret.clear.checked || secret.input.value.trim() !== ""
      : readSetting(field) !== savedSetting(field);
  }

  function validateProviderFields(section, mode) {
    const validationError = (field, message) => {
      settingsControls[field].focus();
      throw new Error(message);
    };
    const validateUrl = (field, label) => {
      let url;
      try {
        url = new URL(readSetting(field));
      } catch {
        validationError(field, `请填写${label}，地址需以 http:// 或 https:// 开头。`);
      }
      if (!["http:", "https:"].includes(url.protocol) || !url.hostname || url.username || url.password) {
        validationError(field, `${label}必须是有效的 HTTP 或 HTTPS 地址，不能包含账号和密码。`);
      }
    };
    if (section === "rerank" && mode === "api") {
      validateUrl("rerank_api_endpoint", "完整重排接口地址");
      if (!readSetting("rerank_api_model")) {
        validationError("rerank_api_model", "请填写服务商提供的 API 模型名称。");
      }
    }
    if (section === "parsing" && mode === "mineru_local") {
      validateUrl("mineru_local_url", "本地 MinerU 完整接口地址");
    }
    if (section === "parsing" && mode === "mineru_cloud") {
      validateUrl("mineru_cloud_url", "云端 API 根地址");
      const token = secretControls.find((entry) => entry.field === "mineru_api_key");
      if (!state.settings.mineru_api_key_set && !token.input.value.trim()) {
        token.input.focus();
        throw new Error("请填写 MinerU 云端访问令牌，再保存云端解析设置。");
      }
    }
  }

  function collectSettingsPatch(section) {
    const patch = {};
    const mode = readSetting(SETTINGS_SECTIONS[section].provider);
    validateProviderFields(section, mode);
    for (const field of activeSectionFields(section)) {
      const secret = secretControls.find((entry) => entry.field === field);
      if (secret) {
        if (secret.clear.checked && !(field === "mineru_api_key" && mode === "mineru_cloud")) {
          patch[field] = "";
        } else if ((field !== "mineru_api_key" || mode === "mineru_cloud") && secret.input.value.trim()) {
          patch[field] = secret.input.value.trim();
        }
        continue;
      }

      const value = INTEGER_SETTINGS[field]
        ? readIntegerSetting(field, settingsControls[field])
        : readSetting(field);
      if (String(value) !== savedSetting(field)) {
        patch[field] = value;
      }
    }
    return patch;
  }

  function setSettingsBusy(busy) {
    state.settingsBusy = busy;
    syncSettingsActions();
  }

  function syncSettingsActions() {
    const section = state.settingsTab;
    const { label, provider } = SETTINGS_SECTIONS[section];
    const currentMode = savedSetting(provider);
    const dirty = Boolean(state.settings) && activeSectionFields(section).some(settingHasChange);
    const hasEdits = Boolean(state.settings) && allSectionFields(section).some(settingHasChange);
    const otherDirty = Boolean(state.settings) && Object.keys(SETTINGS_SECTIONS).some(
      (key) => key !== section && allSectionFields(key).some(settingHasChange),
    );
    const busy = state.settingsBusy;
    const operationsDisabled = busy || !state.settings;
    elements.settingsFields.disabled = busy;
    elements.settingsClose.disabled = busy;
    for (const tab of settingsTabs) {
      tab.disabled = busy;
      tab.querySelector(".settings-tab-dot").hidden = !state.settings
        || !allSectionFields(tab.dataset.settingsTab).some(settingHasChange);
    }
    if (!elements.settingsSubmit.hasAttribute("aria-busy")) {
      elements.settingsSubmit.textContent = `保存并应用${label}`;
    }
    elements.settingsSubmit.disabled = operationsDisabled || !dirty;
    elements.settingsDiscard.disabled = operationsDisabled || !hasEdits;
    elements.settingsProbe.hidden = section === "embedding" || (section === "rerank" && currentMode === "disabled");
    elements.settingsProbe.disabled = operationsDisabled || dirty;
    if (!elements.settingsProbe.hasAttribute("aria-busy")) {
      elements.settingsProbe.textContent = `测试当前${label}`;
    }
    elements.settingsStopProbe.hidden = !state.settingsProbeAbort;
    elements.testEmbeddingButton.disabled = operationsDisabled;
    elements.restoreEmbeddingButton.disabled = operationsDisabled;
    const probeNotes = {
      rerank: currentMode === "disabled" ? "启用并保存重排后可测试。" : "测试会用固定问题和两条示例片段，不读取集合。",
      parsing: currentMode === "mineru_cloud"
        ? "测试会将固定样例上传到云端，可能计入额度，不会入库。"
        : "测试会解析固定样例文件，不会入库。",
      embedding: "此页保存只更新嵌入。",
    };
    elements.settingsActionHint.textContent = dirty
      ? "有未保存修改。保存只更新此页，并立即应用到新请求。"
      : hasEdits ? "有其他方式的未保存修改。切回对应方式后可以保存。"
        : [probeNotes[section], otherDirty ? "其他标签页还有未保存的修改。" : ""].filter(Boolean).join(" ");
  }

  function applyEmbeddingPreset(presetId) {
    const preset = EMBEDDING_PRESETS[presetId];
    if (!preset) {
      return;
    }
    settingsControls.embedding_provider.value = "api";
    settingsControls.embedding_api_protocol.value = preset.protocol;
    settingsControls.embedding_api_base_url.value = preset.baseUrl;
    settingsControls.embedding_api_model.value = preset.model;
    settingsControls.embedding_api_dimensions.value = String(preset.dimensions);
    updateEmbeddingProviderUi();
    syncSettingsActions();
    setStatus(
      elements.settingsFormStatus,
      `已填入 ${preset.label}。保存后会验证 API 配置。`,
      "info",
    );
    settingsControls.embedding_api_base_url.focus();
  }

  async function handleSettingsSubmit(event) {
    event.preventDefault();
    if (state.settingsBusy || !state.settings || elements.settingsSubmit.disabled) {
      return;
    }
    const section = state.settingsTab;
    const { label } = SETTINGS_SECTIONS[section];
    let patch;
    try {
      patch = collectSettingsPatch(section);
    } catch (error) {
      reportMessage(elements.settingsFormStatus, error.message, "error");
      return;
    }

    if (Object.keys(patch).length === 0) {
      setStatus(elements.settingsFormStatus, "没有需要保存的更改。", "warning");
      return;
    }

    const embeddingChanged = Object.keys(patch).some((field) =>
      EMBEDDING_REINDEX_FIELDS.has(field),
    );
    setButtonBusy(elements.settingsSubmit, true);
    setSettingsBusy(true);
    setStatus(elements.settingsFormStatus, `正在保存${label}；保存成功后对新请求生效…`);

    try {
      const updatedSettings = await apiRequest("/settings", {
        method: "PATCH",
        body: patch,
      });
      const savedFields = activeSectionFields(section);
      state.settings = normalizeSettings(updatedSettings);
      fillSettingsForm(state.settings, section, savedFields);
      const summaries = {
        embedding: `${valueOrDash(activeEmbeddingModel(state.settings))} · ${state.settings.embedding_dimension} 维`,
        rerank: rerankSummary(state.settings),
        parsing: parsingSummary(state.settings),
      };
      let message = `已保存并应用${label}：${summaries[section]}。`;
      if (embeddingChanged) {
        message += "嵌入配置已变化，请重新索引已有集合。";
      }
      setStatus(
        elements.settingsFormStatus,
        message,
        embeddingChanged ? "warning" : "success",
      );
      showFeedback(message, embeddingChanged ? "warning" : "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.settingsFormStatus, error, `${label}未保存，当前配置仍在使用。详情：`);
      }
    } finally {
      setButtonBusy(elements.settingsSubmit, false);
      setSettingsBusy(false);
    }
  }

  function discardSettingsEdits() {
    if (!state.settings || state.settingsBusy) {
      return;
    }
    fillSettingsForm(state.settings, state.settingsTab);
    setStatus(elements.settingsFormStatus, "已撤销本页的未保存修改。");
  }

  async function probeProviderSettings() {
    const section = state.settingsTab;
    if (section === "embedding" || state.settingsBusy || elements.settingsProbe.disabled) {
      return;
    }
    const label = SETTINGS_SECTIONS[section].label;
    const controller = new AbortController();
    state.settingsProbeAbort = controller;
    setButtonBusy(elements.settingsProbe, true);
    setSettingsBusy(true);
    setStatus(elements.settingsFormStatus, `正在用固定样例测试当前${label}…`);
    try {
      const result = await apiRequest(`/settings/${section}/probe`, {
        method: "POST",
        signal: controller.signal,
      });
      const detail = section === "rerank"
        ? `${result.model} 返回了 ${result.scored} 条评分`
        : `${result.fixture} 已提取 ${result.characters} 个非空白字符`;
      setStatus(elements.settingsFormStatus, `${label}测试通过：${detail}，耗时 ${result.latency_ms} 毫秒。`, "success");
    } catch (error) {
      if (controller.signal.aborted) {
        setStatus(elements.settingsFormStatus, "已停止等待。云端已收到的任务可能会继续处理。");
      } else if (!error.sessionExpired) {
        const hint = section === "rerank" ? "请检查所选模型或接口与密钥。" : "请检查解析服务、接口地址与令牌。";
        setStatus(elements.settingsFormStatus, `${label}测试未通过。${hint}详情：${error.message}`, "error");
      }
    } finally {
      state.settingsProbeAbort = null;
      setButtonBusy(elements.settingsProbe, false);
      setSettingsBusy(false);
    }
  }

  function formatMetricValue(value, digits = 4) {
    const number = Number(value);
    if (!Number.isFinite(number)) {
      return "—";
    }
    return number.toFixed(digits).replace(/\.?0+$/, "");
  }

  function renderEmbeddingProbe(sample) {
    const values = Array.isArray(sample)
      ? sample.map(Number).filter((value) => Number.isFinite(value))
      : [];
    if (values.length === 0) {
      elements.embeddingProbeChart.hidden = true;
      elements.embeddingProbeBars.replaceChildren();
      elements.embeddingProbeSampleMeta.textContent = "—";
      return;
    }

    const visibleValues = values.slice(0, 32);
    const maximum = Math.max(...visibleValues.map((value) => Math.abs(value)), Number.EPSILON);
    elements.embeddingProbeBars.replaceChildren(
      ...visibleValues.map((value) => {
        const bar = document.createElement("span");
        bar.className = "embedding-probe-bar";
        bar.dataset.sign = value < 0 ? "negative" : "positive";
        bar.style.setProperty(
          "--bar-height",
          `${Math.max(4, (Math.abs(value) / maximum) * 80)}%`,
        );
        return bar;
      }),
    );
    elements.embeddingProbeBars.setAttribute(
      "aria-label",
      `模型返回的向量样本，展示前 ${visibleValues.length} 项`,
    );
    elements.embeddingProbeSampleMeta.textContent =
      `前 ${visibleValues.length} 项 · 共 ${values.length} 项`;
    elements.embeddingProbeChart.hidden = false;
  }

  function showEmbeddingResult({
    title,
    kicker,
    message,
    tone = "success",
    model,
    dimension,
    latencyMs,
    sample,
    norm,
  }) {
    elements.embeddingResultTitle.textContent = title;
    elements.embeddingResultKicker.textContent = kicker;
    elements.embeddingResultMessage.textContent = message;
    elements.embeddingResultMessage.dataset.tone = tone;
    elements.embeddingResultModel.textContent = valueOrDash(model);

    const numericDimension = Number(dimension);
    elements.embeddingResultDimension.textContent =
      Number.isInteger(numericDimension) && numericDimension >= 0
        ? `${numericDimension} 维`
        : "—";
    elements.embeddingResultNorm.textContent = formatMetricValue(norm);

    const numericLatency = Number(latencyMs);
    elements.embeddingResultLatency.hidden = !Number.isFinite(numericLatency);
    elements.embeddingResultLatency.textContent = Number.isFinite(numericLatency)
      ? `${Math.round(numericLatency)} ms`
      : "—";
    renderEmbeddingProbe(sample);
    openDialog(elements.embeddingResultDialog);
  }

  async function probeSavedEmbedding() {
    if (state.settingsBusy || !state.settings) {
      return;
    }
    setButtonBusy(elements.testEmbeddingButton, true);
    setSettingsBusy(true);
    setStatus(elements.settingsFormStatus, "正在测试最近一次保存的嵌入配置…");

    try {
      const result = await apiRequest("/settings/embedding/probe", {
        method: "POST",
      });
      const message = `已保存模型返回正常向量，延迟 ${Math.round(Number(result?.latency_ms) || 0)} ms。`;
      showEmbeddingResult({
        title: "模型测试通过",
        kicker: "已保存配置 · 实时探针",
        message,
        tone: "success",
        model: result?.model,
        dimension: result?.dimension,
        latencyMs: result?.latency_ms,
        sample: result?.sample,
        norm: result?.norm,
      });
      setStatus(elements.settingsFormStatus, message, "success");
      showFeedback(message, "success");
    } catch (error) {
      if (!error.sessionExpired) {
        showEmbeddingResult({
          title: "模型测试失败",
          kicker: "已保存配置 · 实时探针",
          message: error.message,
          tone: "error",
        });
        setStatus(elements.settingsFormStatus, `模型测试失败：${error.message}`, "error");
      }
    } finally {
      setButtonBusy(elements.testEmbeddingButton, false);
      setSettingsBusy(false);
    }
  }

  async function restoreEmbeddingStartup() {
    if (state.settingsBusy || !state.settings) {
      return;
    }
    const accepted = await confirmDestructive(
      "恢复嵌入启动配置？",
      "将嵌入提供方、模型、接口、维度与密钥恢复到启动时的配置。此页未保存的改动会丢失。",
      "恢复嵌入配置",
    );
    if (!accepted) {
      return;
    }

    setButtonBusy(elements.restoreEmbeddingButton, true);
    setSettingsBusy(true);
    setStatus(elements.settingsFormStatus, "正在恢复嵌入启动配置…");

    try {
      const updatedSettings = await apiRequest("/settings/embedding", {
        method: "DELETE",
      });
      state.settings = normalizeSettings(updatedSettings);
      fillSettingsForm(state.settings, "embedding");
      const model = valueOrDash(activeEmbeddingModel(state.settings));
      const dimension = Number(state.settings.embedding_dimension);
      const dimensionLabel =
        Number.isInteger(dimension) && dimension >= 0 ? `${dimension} 维` : "维度待探测";
      const message = `嵌入配置已恢复为启动值：${model} · ${dimensionLabel}。其他服务设置未改变。`;
      showEmbeddingResult({
        title: "启动配置已恢复",
        kicker: "嵌入范围操作",
        message,
        tone: "success",
        model,
        dimension: state.settings.embedding_dimension,
      });
      setStatus(elements.settingsFormStatus, message, "success");
      showFeedback(message, "success");
    } catch (error) {
      if (!error.sessionExpired) {
        reportError(elements.settingsFormStatus, error, "恢复启动配置失败：");
      }
    } finally {
      setButtonBusy(elements.restoreEmbeddingButton, false);
      setSettingsBusy(false);
    }
  }

  function confirmDestructive(title, message, acceptLabel) {
    if (state.confirmResolver) {
      state.confirmResolver(false);
      state.confirmResolver = null;
    }
    elements.confirmDialogTitle.textContent = title;
    elements.confirmDialogMessage.textContent = message;
    elements.confirmAcceptButton.textContent = acceptLabel;
    openDialog(elements.confirmDialog);
    elements.confirmCancelButton.focus();
    return new Promise((resolve) => {
      state.confirmResolver = resolve;
    });
  }

  function settleConfirmation(accepted) {
    const resolver = state.confirmResolver;
    state.confirmResolver = null;
    closeDialog(elements.confirmDialog);
    if (resolver) {
      resolver(accepted);
    }
  }

  function reportError(target, error, prefix = "") {
    const message = `${prefix}${error.message}`;
    setStatus(target, message, "error");
    showFeedback(message, "error", true);
  }

  function reportMessage(target, message, tone = "info") {
    setStatus(target, message, tone);
    showFeedback(message, tone, tone === "error");
  }

  function bindEvents() {
    elements.signInForm.addEventListener("submit", handleSignIn);
    elements.logoutButton.addEventListener("click", () => {
      endSession("已退出；当前页面中的私有数据已清除。", "info");
    });
    elements.feedbackDismiss.addEventListener("click", () => {
      elements.feedback.hidden = true;
    });
    elements.openSettingsButton.addEventListener("click", () => {
      void openSettings();
    });

    elements.refreshCollectionsButton.addEventListener("click", () => {
      void refreshCollections({ chooseFirst: true, fromButton: true });
    });
    elements.createCollectionForm.addEventListener("submit", handleCreateCollection);
    elements.refreshDocumentsButton.addEventListener("click", () => {
      void loadDocumentPage({ resetPaging: true });
    });
    elements.documentsPreviousButton.addEventListener("click", () => {
      void handlePreviousDocumentPage();
    });
    elements.documentsNextButton.addEventListener("click", () => {
      void handleNextDocumentPage();
    });

    elements.searchForm.addEventListener("submit", handleSearchSubmit);
    elements.refreshSearchButton.addEventListener("click", () => {
      void handleRefreshSearch();
    });
    elements.clearSearchButton.addEventListener("click", clearSearchResults);

    elements.manualDocumentForm.addEventListener("submit", handleManualDocumentInspect);
    elements.deleteSelectedDocumentButton.addEventListener("click", () => {
      if (state.selectedDocument) {
        void deleteDocument(state.selectedDocument.doc_id);
      }
    });
    elements.loadMoreChunksButton.addEventListener("click", () => {
      void loadChunks({ reset: false });
    });

    elements.openDocumentDialogButton.addEventListener("click", openDocumentDialog);
    elements.openUploadDialogButton.addEventListener("click", openUploadDialog);
    elements.deleteCollectionButton.addEventListener("click", () => {
      void requestDeleteCollection();
    });
    elements.documentForm.addEventListener("submit", handleDocumentSubmit);
    elements.uploadForm.addEventListener("submit", handleUploadSubmit);
    elements.settingsForm.addEventListener("submit", handleSettingsSubmit);
    elements.settingsForm.addEventListener("input", () => {
      setStatus(elements.settingsFormStatus, "");
      syncSettingsActions();
    });
    elements.settingsForm.addEventListener("change", (event) => {
      if (["embedding_provider", "embedding_api_protocol", "embedding_local_model"].includes(event.target.name)) {
        updateEmbeddingProviderUi();
      }
      updateProviderPanels();
      syncSettingsActions();
    });
    const tablist = settingsTabs[0].closest('[role="tablist"]');
    for (const tab of settingsTabs) {
      tab.addEventListener("click", () => selectSettingsTab(tab.dataset.settingsTab));
    }
    tablist.addEventListener("keydown", (event) => {
      const index = settingsTabs.indexOf(document.activeElement);
      if (index < 0 || state.settingsBusy) {
        return;
      }
      const count = settingsTabs.length;
      const next = {
        ArrowLeft: (index - 1 + count) % count,
        ArrowRight: (index + 1) % count,
        Home: 0,
        End: count - 1,
      }[event.key];
      if (next !== undefined) {
        event.preventDefault();
        selectSettingsTab(settingsTabs[next].dataset.settingsTab, true);
      }
    });
    elements.settingsDiscard.addEventListener("click", discardSettingsEdits);
    elements.settingsProbe.addEventListener("click", () => void probeProviderSettings());
    elements.settingsStopProbe.addEventListener("click", () => state.settingsProbeAbort?.abort());
    elements.settingsForm.addEventListener("click", (event) => {
      const presetButton = event.target.closest("[data-embedding-preset]");
      if (presetButton) {
        applyEmbeddingPreset(presetButton.dataset.embeddingPreset);
      }
    });
    elements.testEmbeddingButton.addEventListener("click", () => {
      void probeSavedEmbedding();
    });
    elements.restoreEmbeddingButton.addEventListener("click", () => {
      void restoreEmbeddingStartup();
    });

    for (const button of document.querySelectorAll("[data-close-dialog]")) {
      button.addEventListener("click", () => {
        const dialog = document.getElementById(button.dataset.closeDialog);
        if (dialog) {
          closeDialog(dialog);
        }
      });
    }

    for (const secret of secretControls) {
      secret.clear.addEventListener("change", () => {
        handleSecretClearToggle(secret);
      });
    }

    elements.settingsDialog.addEventListener("cancel", (event) => {
      if (state.settingsBusy) {
        event.preventDefault();
      }
    });
    elements.settingsDialog.addEventListener("close", () => {
      for (const secret of secretControls) {
        secret.input.value = "";
        secret.input.disabled = false;
        secret.clear.checked = false;
      }
    });

    elements.confirmCancelButton.addEventListener("click", () => {
      settleConfirmation(false);
    });
    elements.confirmAcceptButton.addEventListener("click", () => {
      settleConfirmation(true);
    });
    elements.confirmDialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      settleConfirmation(false);
    });
    elements.confirmDialog.addEventListener("close", () => {
      if (state.confirmResolver) {
        const resolver = state.confirmResolver;
        state.confirmResolver = null;
        resolver(false);
      }
    });
  }

  function initialize() {
    bindEvents();
    disableWorkspaceControls();
    clearSettingsSensitiveControls();
    setSessionStatus("等待验证", "idle");
    elements.accessKeyInput.focus();
  }

  initialize();
})();
