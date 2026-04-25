"""
Provider response-parsing tests: pin behavior of the dict-vs-JSON-string branch
and the malformed-content/empty-content fallbacks. The SDKs are fully mocked.
"""

import io
from unittest.mock import MagicMock

import pytest
from PIL import Image

from llm_provider_base import MetadataGenerationRequest


def _jpeg_bytes():
    buf = io.BytesIO()
    Image.new("RGB", (8, 8), (10, 20, 30)).save(buf, format="JPEG", quality=80)
    return buf.getvalue()


def _request(**overrides):
    defaults = dict(
        image_data=_jpeg_bytes(),
        uuid="uuid-1",
        provider="test",
        model="test-model",
        api_key=None,
        generate_keywords=True,
        generate_caption=True,
        generate_title=False,
        generate_alt_text=False,
        language="English",
        temperature=0.2,
        max_tokens=None,
        system_prompt=None,
        user_prompt=None,
        submit_keywords=False,
        submit_folder_names=False,
        existing_keywords=None,
    )
    defaults.update(overrides)
    return MetadataGenerationRequest(**defaults)


# ----- Ollama --------------------------------------------------------------


@pytest.fixture
def ollama_provider(mocker):
    """Build an OllamaProvider with the SDK Client mocked out."""
    from llm_provider_ollama import OllamaProvider

    fake_client = MagicMock(name="FakeOllamaClient")
    mocker.patch("llm_provider_ollama.Client", return_value=fake_client)
    provider = OllamaProvider({})
    return provider, fake_client


def test_ollama_dict_response_parsed_through(ollama_provider):
    provider, fake_client = ollama_provider
    fake_client.chat.return_value = {
        "message": {
            "content": '{"keywords": ["Mountain"], "caption": "scene", "title": "T", "alt_text": "A"}'
        }
    }
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.keywords == ["Mountain"]
    assert resp.caption == "scene"


def test_ollama_typed_object_response_parsed(ollama_provider):
    provider, fake_client = ollama_provider
    typed = MagicMock()
    typed.message.content = '{"keywords": ["X"], "caption": "c"}'
    fake_client.chat.return_value = typed
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.keywords == ["X"]


def test_ollama_empty_content_returns_failure(ollama_provider):
    provider, fake_client = ollama_provider
    fake_client.chat.return_value = {"message": {"content": ""}}
    resp = provider.generate_metadata(_request())
    assert resp.success is False
    assert "Empty response" in resp.error


def test_ollama_malformed_json_returns_failure(ollama_provider):
    provider, fake_client = ollama_provider
    fake_client.chat.return_value = {"message": {"content": "{not json"}}
    resp = provider.generate_metadata(_request())
    assert resp.success is False
    assert "JSON parsing error" in resp.error


def test_ollama_sdk_exception_returns_failure(ollama_provider):
    provider, fake_client = ollama_provider
    fake_client.chat.side_effect = RuntimeError("network down")
    resp = provider.generate_metadata(_request())
    assert resp.success is False
    assert "network down" in resp.error


def test_ollama_caption_omitted_when_not_requested(ollama_provider):
    provider, fake_client = ollama_provider
    fake_client.chat.return_value = {
        "message": {"content": '{"keywords": [], "caption": "ignore me"}'}
    }
    resp = provider.generate_metadata(_request(generate_caption=False))
    assert resp.success is True
    assert resp.caption is None


# ----- LMStudio ------------------------------------------------------------


@pytest.fixture
def lmstudio_provider(mocker):
    """Build an LMStudioProvider with the SDK fully mocked."""
    fake_lms = mocker.patch("llm_provider_lmstudio.lms")
    fake_response = MagicMock(name="FakeLMSResponse")
    fake_model = MagicMock(name="FakeLMSModel")
    fake_model.respond.return_value = fake_response
    # Simulate no tokenize attribute so the fallback token-usage path is skipped
    del fake_model.tokenize
    del fake_model.apply_prompt_template

    fake_client = MagicMock(name="FakeLMSClient")
    fake_client.__enter__.return_value = fake_client
    fake_client.__exit__.return_value = False
    fake_client.files.prepare_image.return_value = MagicMock(name="image_handle")
    fake_client.llm.model.return_value = fake_model
    fake_lms.Client.return_value = fake_client
    fake_lms.Chat.return_value = MagicMock(name="FakeChat")

    from llm_provider_lmstudio import LMStudioProvider

    provider = LMStudioProvider({})
    return provider, fake_response


def test_lmstudio_dict_parsed_pass_through(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = {
        "keywords": ["Lake"],
        "caption": "view",
        "title": "T",
        "alt_text": "A",
    }
    fake_response.stats = None
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.keywords == ["Lake"]
    assert resp.caption == "view"


def test_lmstudio_json_string_parsed(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = '{"keywords": ["Lake"], "caption": "view"}'
    fake_response.stats = None
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.keywords == ["Lake"]


def test_lmstudio_malformed_string_returns_failure(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = "not json at all"
    fake_response.stats = None
    resp = provider.generate_metadata(_request())
    assert resp.success is False
    assert "Unexpected non-JSON response" in resp.error


def test_lmstudio_unexpected_type_returns_failure(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = 42  # neither dict nor str
    fake_response.stats = None
    resp = provider.generate_metadata(_request())
    assert resp.success is False
    assert "Unexpected response type" in resp.error


def test_lmstudio_token_usage_from_stats(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = {"keywords": [], "caption": "x"}
    stats = MagicMock()
    stats.prompt_tokens = 12
    stats.completion_tokens = 34
    fake_response.stats = stats
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.input_tokens == 12
    assert resp.output_tokens == 34


def test_lmstudio_zero_tokens_when_no_stats_no_tokenize(lmstudio_provider):
    provider, fake_response = lmstudio_provider
    fake_response.parsed = {"keywords": [], "caption": "x"}
    fake_response.stats = None
    fake_response.usage = None
    resp = provider.generate_metadata(_request())
    assert resp.success is True
    assert resp.input_tokens == 0
    assert resp.output_tokens == 0
