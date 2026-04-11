module Protocol exposing
    ( HeartbeatInfo
    , HeartbeatSlider(..)
    , LerpStrategy(..)
    , NoteInfo
    , NoteSlider(..)
    , OverrideInfo
    , PatchParamMeta
    , ProbeLogEntry
    , ServerMsg(..)
    , SliderRange
    , SliderRanges
    , TierInfo
    , TransitionInfo(..)
    , decodeServerMsg
    , encodeAddNote
    , encodeClearOverride
    , encodeCreateOverride
    , encodeCreatePatch
    , encodeExportConfig
    , encodeGetState
    , encodeHeartbeatSlider
    , encodeImportConfig
    , encodeLerpStrategy
    , encodeNoteSlider
    , encodeOverrideHeartbeat
    , encodePlayPatch
    , encodeRemoveNote
    , encodeRenamePatch
    , encodeResetOverrideParam
    , encodeRevertAll
    , encodeSaveConfig
    , encodeSetHeartbeatString
    , encodeSetMasterVolume
    , encodeSetMuted
    , encodeSetNoteTransition
    , encodeSetPatchParam
    , encodeSetPlayback
    , encodeSetTiers
    , encodeTriggerHeartbeat
    )

import Dict exposing (Dict)
import Json.Decode as D
import Json.Encode as E


type alias PatchParamMeta =
    { name : String
    , description : String
    , min : Float
    , max : Float
    , step : Float
    }


type alias NoteInfo =
    { volume : Float
    , offset : Float
    , transition : TransitionInfo
    }


type alias HeartbeatInfo =
    { name : String
    , command : String
    , resultMode : String
    , playback : String
    , metric : Float
    , overridden : Bool
    , pollIntervalSecs : Float
    , cycleSecs : Float
    , cycleOffsetSecs : Float
    , crossfadeMs : Float
    , phraseGap : Float
    , repeatRate : Float
    , notes : List NoteInfo
    , tiers : List TierInfo
    }


type LerpStrategy
    = Linear Float
    | EaseIn Float
    | EaseOut Float
    | EaseInOut Float
    | Step Float


type TransitionInfo
    = Discrete (List { threshold : Float, patch : String })
    | Gradient { patches : List String, segments : List LerpStrategy }


type alias ProbeLogEntry =
    { timestamp : Float
    , name : String
    , result : String
    , overridden : Bool
    }


type alias TierInfo =
    { threshold : Float
    , label : String
    , color : String
    }


type alias SliderRange =
    { min : Float, max : Float, step : Float }


type alias SliderRanges =
    { masterVolume : SliderRange
    , cycleOffset : SliderRange
    , overrideMetric : SliderRange
    , noteVolume : SliderRange
    , noteOffset : SliderRange
    , segmentIntensity : SliderRange
    , discreteThreshold : SliderRange
    , stepPosition : SliderRange
    , crossfadeMs : SliderRange
    }


type HeartbeatSlider
    = CycleOffset
    | CrossfadeMs
    | PollInterval
    | CycleSecs
    | PhraseGap
    | RepeatRate


type NoteSlider
    = NoteVolume
    | NoteOffset


type alias OverrideInfo =
    { base : String, delta : Dict String Float }



-- Server messages (incoming)


type ServerMsg
    = StateMsg
        { patchParams : List PatchParamMeta
        , library : Dict String (Dict String Float)
        , muted : Bool
        , masterVolume : Float
        , heartbeats : List HeartbeatInfo
        , sliderRanges : SliderRanges
        , overrides : Dict String OverrideInfo
        , configWritable : Bool
        , configPath : Maybe String
        }
    | PatchParamChanged String String Float
    | MuteChanged Bool
    | VolumeChanged String (Maybe Int) Float
    | MetricChanged Int Float
    | OverrideChanged Int (Maybe Float) Bool
    | PlaybackChanged Int String
    | LibraryChanged (Dict String (Dict String Float))
    | OverridesChanged (Dict String OverrideInfo)
    | HeartbeatSliderChanged HeartbeatSlider Int Float
    | HeartbeatStringChanged String Int String
    | NoteSliderChanged NoteSlider Int Int Float
    | NoteTransitionChanged Int Int TransitionInfo
    | NotesChanged Int (List NoteInfo)
    | TiersChanged Int (List TierInfo)
    | ProbeLog ProbeLogEntry
    | ConfigExport String
    | ImportError String
    | ConfigSaved
    | SaveError String
    | Connected
    | Disconnected


decodeServerMsg : String -> Result String ServerMsg
decodeServerMsg raw =
    D.decodeString serverMsgDecoder raw
        |> Result.mapError D.errorToString


serverMsgDecoder : D.Decoder ServerMsg
serverMsgDecoder =
    D.field "type" D.string
        |> D.andThen
            (\t ->
                case t of
                    "state" ->
                        stateDecoder

                    "patch_param_changed" ->
                        patchParamChangedDecoder

                    "mute_changed" ->
                        muteChangedDecoder

                    "volume_changed" ->
                        volumeChangedDecoder

                    "metric_changed" ->
                        metricChangedDecoder

                    "override_changed" ->
                        overrideChangedDecoder

                    "playback_changed" ->
                        playbackChangedDecoder

                    "library_changed" ->
                        libraryChangedDecoder

                    "overrides_changed" ->
                        overridesChangedDecoder

                    "cycle_offset_changed" ->
                        heartbeatSliderChangedDecoder CycleOffset

                    "crossfade_ms_changed" ->
                        heartbeatSliderChangedDecoder CrossfadeMs

                    "poll_interval_changed" ->
                        heartbeatSliderChangedDecoder PollInterval

                    "cycle_secs_changed" ->
                        heartbeatSliderChangedDecoder CycleSecs

                    "phrase_gap_changed" ->
                        heartbeatSliderChangedDecoder PhraseGap

                    "repeat_rate_changed" ->
                        heartbeatSliderChangedDecoder RepeatRate

                    "heartbeat_name_changed" ->
                        heartbeatStringChangedDecoder "name"

                    "heartbeat_command_changed" ->
                        heartbeatStringChangedDecoder "command"

                    "result_mode_changed" ->
                        heartbeatStringChangedDecoder "result_mode"

                    "note_volume_changed" ->
                        noteSliderChangedDecoder NoteVolume

                    "note_offset_changed" ->
                        noteSliderChangedDecoder NoteOffset

                    "note_transition_changed" ->
                        noteTransitionChangedDecoder

                    "notes_changed" ->
                        notesChangedDecoder

                    "tiers_changed" ->
                        tiersChangedDecoder

                    "probe_log" ->
                        probeLogDecoder

                    "config_export" ->
                        configExportDecoder

                    "import_error" ->
                        importErrorDecoder

                    "config_saved" ->
                        D.succeed ConfigSaved

                    "save_error" ->
                        saveErrorDecoder

                    "connected" ->
                        D.succeed Connected

                    "disconnected" ->
                        D.succeed Disconnected

                    _ ->
                        D.fail ("Unknown server message type: " ++ t)
            )



-- Decoders


stateDecoder : D.Decoder ServerMsg
stateDecoder =
    D.map5
        (\pp lib muted mv hbs ->
            \sr ovr cw cp ->
                StateMsg
                    { patchParams = pp
                    , library = lib
                    , muted = muted
                    , masterVolume = mv
                    , heartbeats = hbs
                    , sliderRanges = sr
                    , overrides = ovr
                    , configWritable = cw
                    , configPath = cp
                    }
        )
        (D.field "patch_params" (D.list patchParamMetaDecoder))
        (D.field "library" libraryDecoder)
        (D.field "muted" D.bool)
        (D.field "master_volume" D.float)
        (D.field "heartbeats" (D.list heartbeatInfoDecoder))
        |> D.andThen
            (\buildState ->
                D.map4 buildState
                    (D.field "slider_ranges" sliderRangesDecoder)
                    (D.field "overrides" overridesDecoder)
                    (D.oneOf
                        [ D.field "config_writable" D.bool
                        , D.succeed False
                        ]
                    )
                    (D.maybe (D.field "config_path" D.string))
            )


patchParamMetaDecoder : D.Decoder PatchParamMeta
patchParamMetaDecoder =
    D.map5
        (\n d mn mx s ->
            { name = n
            , description = d
            , min = mn
            , max = mx
            , step = s
            }
        )
        (D.field "name" D.string)
        (D.field "description" D.string)
        (D.field "min" D.float)
        (D.field "max" D.float)
        (D.field "step" D.float)


libraryDecoder : D.Decoder (Dict String (Dict String Float))
libraryDecoder =
    D.dict (D.dict D.float)


noteInfoDecoder : D.Decoder NoteInfo
noteInfoDecoder =
    D.map3 NoteInfo
        (D.field "volume" D.float)
        (D.field "offset" D.float)
        (D.field "transition" transitionDecoder)


heartbeatInfoDecoder : D.Decoder HeartbeatInfo
heartbeatInfoDecoder =
    D.map8
        (\name command resultMode playback metric overridden pollInterval cycleSecs ->
            \cycleOffset crossfade phraseGap repeatRate notes tiers ->
                HeartbeatInfo name
                    command
                    resultMode
                    playback
                    metric
                    overridden
                    pollInterval
                    cycleSecs
                    cycleOffset
                    crossfade
                    phraseGap
                    repeatRate
                    notes
                    tiers
        )
        (D.field "name" D.string)
        (D.oneOf [ D.field "command" D.string, D.succeed "" ])
        (D.oneOf [ D.field "result_mode" D.string, D.succeed "stdout" ])
        (D.oneOf [ D.field "playback" D.string, D.succeed "clock" ])
        (D.field "metric" D.float)
        (D.field "overridden" D.bool)
        (D.oneOf [ D.field "poll_interval_secs" D.float, D.succeed 10.0 ])
        (D.oneOf [ D.field "cycle_secs" D.float, D.succeed 15.0 ])
        |> D.andThen
            (\build ->
                D.map6 build
                    (D.oneOf [ D.field "cycle_offset_secs" D.float, D.succeed 0.0 ])
                    (D.oneOf [ D.field "crossfade_ms" D.float, D.succeed 6.0 ])
                    (D.oneOf [ D.field "phrase_gap" D.float, D.succeed 0.0 ])
                    (D.oneOf [ D.field "repeat_rate" D.float, D.succeed 1.0 ])
                    (D.field "notes" (D.list noteInfoDecoder))
                    (D.oneOf [ D.field "tiers" (D.list tierInfoDecoder), D.succeed [] ])
            )


sliderRangeDecoder : D.Decoder SliderRange
sliderRangeDecoder =
    D.map3 SliderRange
        (D.field "min" D.float)
        (D.field "max" D.float)
        (D.field "step" D.float)


sliderRangesDecoder : D.Decoder SliderRanges
sliderRangesDecoder =
    D.map6
        (\mv co om nv no si ->
            \dt sp cf ->
                SliderRanges mv co om nv no si dt sp cf
        )
        (D.field "master_volume" sliderRangeDecoder)
        (D.field "cycle_offset" sliderRangeDecoder)
        (D.field "override_metric" sliderRangeDecoder)
        (D.field "note_volume" sliderRangeDecoder)
        (D.field "note_offset" sliderRangeDecoder)
        (D.field "segment_intensity" sliderRangeDecoder)
        |> D.andThen
            (\build ->
                D.map3 build
                    (D.field "discrete_threshold" sliderRangeDecoder)
                    (D.field "step_position" sliderRangeDecoder)
                    (D.field "crossfade_ms" sliderRangeDecoder)
            )


lerpStrategyDecoder : D.Decoder LerpStrategy
lerpStrategyDecoder =
    D.field "strategy" D.string
        |> D.andThen
            (\s ->
                case s of
                    "linear" ->
                        D.map Linear
                            (D.oneOf
                                [ D.field "intensity" D.float
                                , D.succeed 2.0
                                ]
                            )

                    "ease-in" ->
                        D.map EaseIn
                            (D.oneOf
                                [ D.field "intensity" D.float
                                , D.succeed 2.0
                                ]
                            )

                    "ease-out" ->
                        D.map EaseOut
                            (D.oneOf
                                [ D.field "intensity" D.float
                                , D.succeed 2.0
                                ]
                            )

                    "ease-in-out" ->
                        D.map EaseInOut
                            (D.oneOf
                                [ D.field "intensity" D.float
                                , D.succeed 2.0
                                ]
                            )

                    "step" ->
                        D.map Step
                            (D.oneOf
                                [ D.field "intensity" D.float
                                , D.succeed 0.5
                                ]
                            )

                    _ ->
                        D.fail ("Unknown lerp strategy: " ++ s)
            )


transitionDecoder : D.Decoder TransitionInfo
transitionDecoder =
    D.field "type" D.string
        |> D.andThen
            (\t ->
                case t of
                    "discrete" ->
                        D.map Discrete
                            (D.field "states"
                                (D.list
                                    (D.map2 (\th p -> { threshold = th, patch = p })
                                        (D.field "threshold" D.float)
                                        (D.field "patch" D.string)
                                    )
                                )
                            )

                    "gradient" ->
                        D.map2 (\ps segs -> Gradient { patches = ps, segments = segs })
                            (D.field "patches" (D.list D.string))
                            (D.oneOf
                                [ D.field "segments" (D.list lerpStrategyDecoder)
                                , D.succeed []
                                ]
                            )

                    _ ->
                        D.fail ("Unknown transition type: " ++ t)
            )


patchParamChangedDecoder : D.Decoder ServerMsg
patchParamChangedDecoder =
    D.map3 PatchParamChanged
        (D.field "patch_name" D.string)
        (D.field "param" D.string)
        (D.field "value" D.float)


muteChangedDecoder : D.Decoder ServerMsg
muteChangedDecoder =
    D.map MuteChanged (D.field "muted" D.bool)


volumeChangedDecoder : D.Decoder ServerMsg
volumeChangedDecoder =
    D.map3 VolumeChanged
        (D.field "layer" D.string)
        (D.maybe (D.field "index" D.int))
        (D.field "volume" D.float)


metricChangedDecoder : D.Decoder ServerMsg
metricChangedDecoder =
    D.map2 MetricChanged
        (D.field "index" D.int)
        (D.field "value" D.float)


overrideChangedDecoder : D.Decoder ServerMsg
overrideChangedDecoder =
    D.map3 OverrideChanged
        (D.field "index" D.int)
        (D.maybe (D.field "value" D.float))
        (D.field "overridden" D.bool)


playbackChangedDecoder : D.Decoder ServerMsg
playbackChangedDecoder =
    D.map2 PlaybackChanged
        (D.field "index" D.int)
        (D.field "value" D.string)


libraryChangedDecoder : D.Decoder ServerMsg
libraryChangedDecoder =
    D.map LibraryChanged (D.field "library" libraryDecoder)


overrideInfoDecoder : D.Decoder OverrideInfo
overrideInfoDecoder =
    D.map2 OverrideInfo
        (D.field "base" D.string)
        (D.field "delta" (D.dict D.float))


overridesDecoder : D.Decoder (Dict String OverrideInfo)
overridesDecoder =
    D.dict overrideInfoDecoder


overridesChangedDecoder : D.Decoder ServerMsg
overridesChangedDecoder =
    D.map OverridesChanged (D.field "overrides" overridesDecoder)


heartbeatSliderChangedDecoder : HeartbeatSlider -> D.Decoder ServerMsg
heartbeatSliderChangedDecoder slider =
    D.map2 (HeartbeatSliderChanged slider)
        (D.field "index" D.int)
        (D.field "value" D.float)


heartbeatStringChangedDecoder : String -> D.Decoder ServerMsg
heartbeatStringChangedDecoder field =
    D.map2 (HeartbeatStringChanged field)
        (D.field "index" D.int)
        (D.field "value" D.string)


noteSliderChangedDecoder : NoteSlider -> D.Decoder ServerMsg
noteSliderChangedDecoder slider =
    D.map3 (NoteSliderChanged slider)
        (D.field "index" D.int)
        (D.field "note" D.int)
        (D.field "value" D.float)


noteTransitionChangedDecoder : D.Decoder ServerMsg
noteTransitionChangedDecoder =
    D.map3 NoteTransitionChanged
        (D.field "index" D.int)
        (D.field "note" D.int)
        (D.field "transition" transitionDecoder)


notesChangedDecoder : D.Decoder ServerMsg
notesChangedDecoder =
    D.map2 NotesChanged
        (D.field "index" D.int)
        (D.field "notes" (D.list noteInfoDecoder))


tierInfoDecoder : D.Decoder TierInfo
tierInfoDecoder =
    D.map3 TierInfo
        (D.field "threshold" D.float)
        (D.field "label" D.string)
        (D.field "color" D.string)


tiersChangedDecoder : D.Decoder ServerMsg
tiersChangedDecoder =
    D.map2 TiersChanged
        (D.field "index" D.int)
        (D.field "tiers" (D.list tierInfoDecoder))


probeLogDecoder : D.Decoder ServerMsg
probeLogDecoder =
    D.map4
        (\ts name result overridden ->
            ProbeLog
                { timestamp = ts
                , name = name
                , result = result
                , overridden = overridden
                }
        )
        (D.field "timestamp" D.float)
        (D.field "name" D.string)
        (D.field "result" D.string)
        (D.field "overridden" D.bool)


configExportDecoder : D.Decoder ServerMsg
configExportDecoder =
    D.map ConfigExport (D.field "toml" D.string)


importErrorDecoder : D.Decoder ServerMsg
importErrorDecoder =
    D.map ImportError (D.field "message" D.string)


saveErrorDecoder : D.Decoder ServerMsg
saveErrorDecoder =
    D.map SaveError (D.field "message" D.string)



-- Client messages (outgoing)


encodeGetState : String
encodeGetState =
    E.object [ ( "type", E.string "get_state" ) ]
        |> E.encode 0


encodeSetPatchParam : String -> String -> Float -> String
encodeSetPatchParam patchName param value =
    E.object
        [ ( "type", E.string "set_patch_param" )
        , ( "patch_name", E.string patchName )
        , ( "param", E.string param )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeNoteSlider : NoteSlider -> Int -> Int -> Float -> String
encodeNoteSlider slider index note value =
    let
        msgType =
            case slider of
                NoteVolume ->
                    "set_note_volume"

                NoteOffset ->
                    "set_note_offset"
    in
    E.object
        [ ( "type", E.string msgType )
        , ( "index", E.int index )
        , ( "note", E.int note )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeSetNoteTransition : Int -> Int -> TransitionInfo -> String
encodeSetNoteTransition index note trans =
    E.object
        [ ( "type", E.string "set_note_transition" )
        , ( "index", E.int index )
        , ( "note", E.int note )
        , ( "transition", encodeTransition trans )
        ]
        |> E.encode 0


encodeAddNote : Int -> String
encodeAddNote index =
    E.object
        [ ( "type", E.string "add_note" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeRemoveNote : Int -> Int -> String
encodeRemoveNote index note =
    E.object
        [ ( "type", E.string "remove_note" )
        , ( "index", E.int index )
        , ( "note", E.int note )
        ]
        |> E.encode 0


encodeOverrideHeartbeat : Int -> Float -> String
encodeOverrideHeartbeat index value =
    E.object
        [ ( "type", E.string "override_heartbeat" )
        , ( "index", E.int index )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeClearOverride : Int -> String
encodeClearOverride index =
    E.object
        [ ( "type", E.string "clear_override" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeTriggerHeartbeat : Int -> String
encodeTriggerHeartbeat index =
    E.object
        [ ( "type", E.string "trigger_heartbeat" )
        , ( "index", E.int index )
        ]
        |> E.encode 0


encodeSetPlayback : Int -> String -> String
encodeSetPlayback index value =
    E.object
        [ ( "type", E.string "set_playback" )
        , ( "index", E.int index )
        , ( "value", E.string value )
        ]
        |> E.encode 0


encodeSetMuted : Bool -> String
encodeSetMuted muted =
    E.object
        [ ( "type", E.string "set_muted" )
        , ( "muted", E.bool muted )
        ]
        |> E.encode 0


encodeSetMasterVolume : Float -> String
encodeSetMasterVolume vol =
    E.object
        [ ( "type", E.string "set_master_volume" )
        , ( "volume", E.float vol )
        ]
        |> E.encode 0


encodeRevertAll : String
encodeRevertAll =
    E.object [ ( "type", E.string "revert_all" ) ]
        |> E.encode 0


encodeExportConfig : String
encodeExportConfig =
    E.object [ ( "type", E.string "export_config" ) ]
        |> E.encode 0


encodeSaveConfig : String
encodeSaveConfig =
    E.object [ ( "type", E.string "save_config" ) ]
        |> E.encode 0


encodeImportConfig : String -> String
encodeImportConfig text =
    E.object
        [ ( "type", E.string "import_config" )
        , ( "text", E.string text )
        ]
        |> E.encode 0


encodeHeartbeatSlider : HeartbeatSlider -> Int -> Float -> String
encodeHeartbeatSlider slider index value =
    let
        msgType =
            case slider of
                CycleOffset ->
                    "set_cycle_offset"

                CrossfadeMs ->
                    "set_crossfade_ms"

                PollInterval ->
                    "set_poll_interval"

                CycleSecs ->
                    "set_cycle_secs"

                PhraseGap ->
                    "set_phrase_gap"

                RepeatRate ->
                    "set_repeat_rate"
    in
    E.object
        [ ( "type", E.string msgType )
        , ( "index", E.int index )
        , ( "value", E.float value )
        ]
        |> E.encode 0


encodeSetHeartbeatString : String -> Int -> String -> String
encodeSetHeartbeatString msgType index value =
    E.object
        [ ( "type", E.string msgType )
        , ( "index", E.int index )
        , ( "value", E.string value )
        ]
        |> E.encode 0


encodeLerpStrategy : LerpStrategy -> E.Value
encodeLerpStrategy strat =
    case strat of
        Linear intensity ->
            E.object
                [ ( "strategy", E.string "linear" )
                , ( "intensity", E.float intensity )
                ]

        EaseIn intensity ->
            E.object
                [ ( "strategy", E.string "ease-in" )
                , ( "intensity", E.float intensity )
                ]

        EaseOut intensity ->
            E.object
                [ ( "strategy", E.string "ease-out" )
                , ( "intensity", E.float intensity )
                ]

        EaseInOut intensity ->
            E.object
                [ ( "strategy", E.string "ease-in-out" )
                , ( "intensity", E.float intensity )
                ]

        Step intensity ->
            E.object
                [ ( "strategy", E.string "step" )
                , ( "intensity", E.float intensity )
                ]


encodeTransition : TransitionInfo -> E.Value
encodeTransition trans =
    case trans of
        Discrete states ->
            E.object
                [ ( "type", E.string "discrete" )
                , ( "states"
                  , E.list
                        (\s ->
                            E.object
                                [ ( "threshold", E.float s.threshold )
                                , ( "patch", E.string s.patch )
                                ]
                        )
                        states
                  )
                ]

        Gradient info ->
            E.object
                [ ( "type", E.string "gradient" )
                , ( "patches", E.list E.string info.patches )
                , ( "segments", E.list encodeLerpStrategy info.segments )
                ]


encodeCreatePatch : String -> String
encodeCreatePatch name =
    E.object
        [ ( "type", E.string "create_patch" )
        , ( "name", E.string name )
        ]
        |> E.encode 0


encodeCreateOverride : String -> String -> String
encodeCreateOverride base name =
    E.object
        [ ( "type", E.string "create_override" )
        , ( "base", E.string base )
        , ( "name", E.string name )
        ]
        |> E.encode 0


encodeRenamePatch : String -> String -> String
encodeRenamePatch oldName newName =
    E.object
        [ ( "type", E.string "rename_patch" )
        , ( "old_name", E.string oldName )
        , ( "new_name", E.string newName )
        ]
        |> E.encode 0


encodeResetOverrideParam : String -> String -> String
encodeResetOverrideParam patchName param =
    E.object
        [ ( "type", E.string "reset_override_param" )
        , ( "patch_name", E.string patchName )
        , ( "param", E.string param )
        ]
        |> E.encode 0


encodeSetTiers : Int -> List TierInfo -> String
encodeSetTiers index tiers =
    E.object
        [ ( "type", E.string "set_tiers" )
        , ( "index", E.int index )
        , ( "tiers"
          , E.list
                (\t ->
                    E.object
                        [ ( "threshold", E.float t.threshold )
                        , ( "label", E.string t.label )
                        , ( "color", E.string t.color )
                        ]
                )
                tiers
          )
        ]
        |> E.encode 0


encodePlayPatch : String -> String
encodePlayPatch patchName =
    E.object
        [ ( "type", E.string "play_patch" )
        , ( "patch_name", E.string patchName )
        ]
        |> E.encode 0
