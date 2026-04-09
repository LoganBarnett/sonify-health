module Protocol exposing
    ( HeartbeatInfo
    , NoteInfo
    , PatchParamMeta
    , ProbeLogEntry
    , ServerMsg(..)
    , TransitionInfo(..)
    , decodeServerMsg
    , encodeAddNote
    , encodeClearOverride
    , encodeExportConfig
    , encodeGetState
    , encodeImportConfig
    , encodeOverrideHeartbeat
    , encodeRemoveNote
    , encodeRevertAll
    , encodeSetCycleOffset
    , encodeSetHeartbeatLoop
    , encodeSetMasterVolume
    , encodeSetMuted
    , encodeSetNoteOffset
    , encodeSetNoteTransition
    , encodeSetNoteVolume
    , encodeSetPatchParam
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
    , continuous : Bool
    , metric : Float
    , overridden : Bool
    , cycleOffsetSecs : Float
    , notes : List NoteInfo
    }


type TransitionInfo
    = Discrete (List { threshold : Float, patch : String })
    | Gradient { patches : List String, curve : Float }


type alias ProbeLogEntry =
    { timestamp : Float
    , name : String
    , result : String
    , overridden : Bool
    }



-- Server messages (incoming)


type ServerMsg
    = StateMsg
        { patchParams : List PatchParamMeta
        , library : Dict String (Dict String Float)
        , muted : Bool
        , masterVolume : Float
        , heartbeatLoop : Bool
        , heartbeats : List HeartbeatInfo
        }
    | PatchParamChanged String String Float
    | MuteChanged Bool
    | VolumeChanged String (Maybe Int) Float
    | MetricChanged Int Float
    | OverrideChanged Int (Maybe Float) Bool
    | HeartbeatLoopChanged Bool
    | LibraryChanged (Dict String (Dict String Float))
    | CycleOffsetChanged Int Float
    | NoteVolumeChanged Int Int Float
    | NoteOffsetChanged Int Int Float
    | NoteTransitionChanged Int Int TransitionInfo
    | NotesChanged Int (List NoteInfo)
    | ProbeLog ProbeLogEntry
    | ConfigExport (Dict String (Dict String Float))
    | ImportError String
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

                    "heartbeat_loop_changed" ->
                        heartbeatLoopChangedDecoder

                    "library_changed" ->
                        libraryChangedDecoder

                    "cycle_offset_changed" ->
                        cycleOffsetChangedDecoder

                    "note_volume_changed" ->
                        noteVolumeChangedDecoder

                    "note_offset_changed" ->
                        noteOffsetChangedDecoder

                    "note_transition_changed" ->
                        noteTransitionChangedDecoder

                    "notes_changed" ->
                        notesChangedDecoder

                    "probe_log" ->
                        probeLogDecoder

                    "config_export" ->
                        configExportDecoder

                    "import_error" ->
                        importErrorDecoder

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
    D.map6
        (\pp lib muted mv hbLoop hbs ->
            StateMsg
                { patchParams = pp
                , library = lib
                , muted = muted
                , masterVolume = mv
                , heartbeatLoop = hbLoop
                , heartbeats = hbs
                }
        )
        (D.field "patch_params" (D.list patchParamMetaDecoder))
        (D.field "library" libraryDecoder)
        (D.field "muted" D.bool)
        (D.field "master_volume" D.float)
        (D.field "heartbeat_loop" D.bool)
        (D.field "heartbeats" (D.list heartbeatInfoDecoder))


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
    D.map6 HeartbeatInfo
        (D.field "name" D.string)
        (D.field "continuous" D.bool)
        (D.field "metric" D.float)
        (D.field "overridden" D.bool)
        (D.field "cycle_offset_secs" D.float)
        (D.field "notes" (D.list noteInfoDecoder))


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
                        D.map2 (\ps c -> Gradient { patches = ps, curve = c })
                            (D.field "patches" (D.list D.string))
                            (D.field "curve" D.float)

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


heartbeatLoopChangedDecoder : D.Decoder ServerMsg
heartbeatLoopChangedDecoder =
    D.map HeartbeatLoopChanged (D.field "enabled" D.bool)


libraryChangedDecoder : D.Decoder ServerMsg
libraryChangedDecoder =
    D.map LibraryChanged (D.field "library" libraryDecoder)


cycleOffsetChangedDecoder : D.Decoder ServerMsg
cycleOffsetChangedDecoder =
    D.map2 CycleOffsetChanged
        (D.field "index" D.int)
        (D.field "value" D.float)


noteVolumeChangedDecoder : D.Decoder ServerMsg
noteVolumeChangedDecoder =
    D.map3 NoteVolumeChanged
        (D.field "index" D.int)
        (D.field "note" D.int)
        (D.field "volume" D.float)


noteOffsetChangedDecoder : D.Decoder ServerMsg
noteOffsetChangedDecoder =
    D.map3 NoteOffsetChanged
        (D.field "index" D.int)
        (D.field "note" D.int)
        (D.field "offset" D.float)


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
    D.map ConfigExport (D.field "library" libraryDecoder)


importErrorDecoder : D.Decoder ServerMsg
importErrorDecoder =
    D.map ImportError (D.field "message" D.string)



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


encodeSetNoteVolume : Int -> Int -> Float -> String
encodeSetNoteVolume index note volume =
    E.object
        [ ( "type", E.string "set_note_volume" )
        , ( "index", E.int index )
        , ( "note", E.int note )
        , ( "volume", E.float volume )
        ]
        |> E.encode 0


encodeSetNoteOffset : Int -> Int -> Float -> String
encodeSetNoteOffset index note offset =
    E.object
        [ ( "type", E.string "set_note_offset" )
        , ( "index", E.int index )
        , ( "note", E.int note )
        , ( "offset", E.float offset )
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


encodeTriggerHeartbeat : String
encodeTriggerHeartbeat =
    E.object [ ( "type", E.string "trigger_heartbeat" ) ]
        |> E.encode 0


encodeSetHeartbeatLoop : Bool -> String
encodeSetHeartbeatLoop enabled =
    E.object
        [ ( "type", E.string "set_heartbeat_loop" )
        , ( "enabled", E.bool enabled )
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


encodeImportConfig : String -> String
encodeImportConfig text =
    E.object
        [ ( "type", E.string "import_config" )
        , ( "text", E.string text )
        ]
        |> E.encode 0


encodeSetCycleOffset : Int -> Float -> String
encodeSetCycleOffset index value =
    E.object
        [ ( "type", E.string "set_cycle_offset" )
        , ( "index", E.int index )
        , ( "value", E.float value )
        ]
        |> E.encode 0


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
                , ( "curve", E.float info.curve )
                ]
