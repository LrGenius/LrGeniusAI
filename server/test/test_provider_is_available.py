"""
Health-check tests for provider is_available().
These run on the /health hot path, so false positives/negatives have
user-visible consequences.
"""


# ----- Ollama --------------------------------------------------------------


def test_ollama_is_available_returns_false_when_sdk_missing(mocker):
    mocker.patch("llm_provider_ollama.Client", None)
    from llm_provider_ollama import OllamaProvider

    provider = OllamaProvider({})
    assert provider.is_available() is False


def test_ollama_is_available_returns_false_when_sdk_raises(mocker):
    fake_client_class = mocker.MagicMock()
    fake_client_class.return_value.list.side_effect = ConnectionError(
        "connection refused"
    )
    mocker.patch("llm_provider_ollama.Client", fake_client_class)

    from llm_provider_ollama import OllamaProvider

    provider = OllamaProvider({})
    assert provider.is_available() is False


def test_ollama_is_available_returns_true_when_sdk_responds(mocker):
    fake_client_class = mocker.MagicMock()
    fake_client_class.return_value.list.return_value = {"models": []}
    mocker.patch("llm_provider_ollama.Client", fake_client_class)

    from llm_provider_ollama import OllamaProvider

    provider = OllamaProvider({})
    assert provider.is_available() is True


# ----- LMStudio ------------------------------------------------------------


def test_lmstudio_is_available_rejects_host_without_colon(mocker):
    fake_lms = mocker.patch("llm_provider_lmstudio.lms")
    from llm_provider_lmstudio import LMStudioProvider

    provider = LMStudioProvider({"base_url": "no-colon-here"})
    assert provider.is_available() is False
    # Critically, the SDK validation must NOT have been invoked when the host
    # is malformed — the cheap pre-check short-circuits.
    fake_lms.Client.is_valid_api_host.assert_not_called()


def test_lmstudio_is_available_rejects_empty_host(mocker):
    mocker.patch("llm_provider_lmstudio.lms")
    from llm_provider_lmstudio import LMStudioProvider

    provider = LMStudioProvider({"base_url": ""})
    assert provider.is_available() is False


def test_lmstudio_is_available_returns_false_on_sdk_exception(mocker):
    fake_lms = mocker.patch("llm_provider_lmstudio.lms")
    fake_lms.Client.is_valid_api_host.side_effect = RuntimeError("sdk borked")
    from llm_provider_lmstudio import LMStudioProvider

    provider = LMStudioProvider({"base_url": "host:1234"})
    assert provider.is_available() is False


def test_lmstudio_is_available_returns_sdk_result(mocker):
    fake_lms = mocker.patch("llm_provider_lmstudio.lms")
    fake_lms.Client.is_valid_api_host.return_value = True
    from llm_provider_lmstudio import LMStudioProvider

    provider = LMStudioProvider({"base_url": "host:1234"})
    assert provider.is_available() is True
