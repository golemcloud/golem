from typing import TypeVar, Generic, Union, Optional, Protocol, Tuple, List, Any, Self
from types import TracebackType
from enum import Flag, Enum, auto
from dataclasses import dataclass
from abc import abstractmethod
import weakref

from ..types import Result, Ok, Err, Some


@dataclass
class ListVoicesRequest:
    language_code: Optional[str]

@dataclass
class Unused:
    unsed: Optional[str]

@dataclass
class AdvancedVoiceOptions:
    low_latency_journey_synthesis: Optional[bool]

class CustomPronunciationParamsPhoneticEncoding(Enum):
    PHONETIC_ENCODING_UNSPECIFIED = 0
    PHONETIC_ENCODING_IPA = 1
    PHONETIC_ENCODING_X_SAMPA = 2
    PHONETIC_ENCODING_JAPANESE_YOMIGANA = 3
    PHONETIC_ENCODING_PINYIN = 4

@dataclass
class CustomPronunciationParams:
    phrase: Optional[str]
    phonetic_encoding: Optional[CustomPronunciationParamsPhoneticEncoding]
    pronunciation: Optional[str]

@dataclass
class CustomPronunciations:
    pronunciations: List[CustomPronunciationParams]

@dataclass
class MultiSpeakerMarkupTurn:
    speaker: Optional[str]
    text: Optional[str]

@dataclass
class MultiSpeakerMarkup:
    turns: List[MultiSpeakerMarkupTurn]


@dataclass
class SynthesisInputInputSource_Text:
    value: Optional[str]


@dataclass
class SynthesisInputInputSource_Markup:
    value: Optional[str]


@dataclass
class SynthesisInputInputSource_Ssml:
    value: Optional[str]


@dataclass
class SynthesisInputInputSource_MultiSpeakerMarkup:
    value: Optional[MultiSpeakerMarkup]


SynthesisInputInputSource = Union[SynthesisInputInputSource_Text, SynthesisInputInputSource_Markup, SynthesisInputInputSource_Ssml, SynthesisInputInputSource_MultiSpeakerMarkup]


@dataclass
class SynthesisInput:
    text: Optional[str]
    markup: Optional[str]
    ssml: Optional[str]
    multi_speaker_markup: Optional[MultiSpeakerMarkup]
    custom_pronunciations: Optional[CustomPronunciations]

class CustomVoiceParamsReportedUsage(Enum):
    REPORTED_USAGE_UNSPECIFIED = 0
    REALTIME = 1
    OFFLINE = 2

@dataclass
class CustomVoiceParams:
    model: Optional[str]
    reported_usage: Optional[CustomVoiceParamsReportedUsage]

@dataclass
class VoiceCloneParams:
    voice_cloning_key: Optional[str]

@dataclass
class SynthesizeSpeechResponse:
    audio_content: Optional[bytes]


@dataclass
class StreamingSynthesisInputInputSource_Text:
    value: Optional[str]


@dataclass
class StreamingSynthesisInputInputSource_Markup:
    value: Optional[str]


StreamingSynthesisInputInputSource = Union[StreamingSynthesisInputInputSource_Text, StreamingSynthesisInputInputSource_Markup]


@dataclass
class StreamingSynthesisInput:
    text: Optional[str]
    markup: Optional[str]

@dataclass
class StreamingSynthesizeResponse:
    audio_content: Optional[bytes]

class SsmlVoiceGender(Enum):
    SSML_VOICE_GENDER_UNSPECIFIED = 0
    MALE = 1
    FEMALE = 2
    NEUTRAL = 3

@dataclass
class Voice:
    language_codes: List[str]
    name: Optional[str]
    ssml_gender: Optional[SsmlVoiceGender]
    natural_sample_rate_hertz: Optional[int]

@dataclass
class ListVoicesResponse:
    voices: List[Voice]

@dataclass
class VoiceSelectionParams:
    language_code: Optional[str]
    name: Optional[str]
    ssml_gender: Optional[SsmlVoiceGender]
    custom_voice: Optional[CustomVoiceParams]
    voice_clone: Optional[VoiceCloneParams]

class AudioEncoding(Enum):
    AUDIO_ENCODING_UNSPECIFIED = 0
    LINEAR16 = 1
    MP3 = 2
    OGG_OPUS = 3
    MULAW = 4
    ALAW = 5
    PCM = 6

@dataclass
class AudioConfig:
    audio_encoding: Optional[AudioEncoding]
    speaking_rate: Optional[float]
    pitch: Optional[float]
    volume_gain_db: Optional[float]
    sample_rate_hertz: Optional[int]
    effects_profile_id: List[str]

@dataclass
class SynthesizeSpeechRequest:
    input: Optional[SynthesisInput]
    voice: Optional[VoiceSelectionParams]
    audio_config: Optional[AudioConfig]
    advanced_voice_options: Optional[AdvancedVoiceOptions]

@dataclass
class StreamingAudioConfig:
    audio_encoding: Optional[AudioEncoding]
    sample_rate_hertz: Optional[int]
    speaking_rate: Optional[float]

@dataclass
class StreamingSynthesizeConfig:
    voice: Optional[VoiceSelectionParams]
    streaming_audio_config: Optional[StreamingAudioConfig]
    custom_pronunciations: Optional[CustomPronunciations]


@dataclass
class StreamingSynthesizeRequestStreamingRequest_StreamingConfig:
    value: Optional[StreamingSynthesizeConfig]


@dataclass
class StreamingSynthesizeRequestStreamingRequest_Input:
    value: Optional[StreamingSynthesisInput]


StreamingSynthesizeRequestStreamingRequest = Union[StreamingSynthesizeRequestStreamingRequest_StreamingConfig, StreamingSynthesizeRequestStreamingRequest_Input]


@dataclass
class StreamingSynthesizeRequest:
    streaming_config: Optional[StreamingSynthesizeConfig]
    input: Optional[StreamingSynthesisInput]


