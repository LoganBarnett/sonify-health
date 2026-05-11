module Main exposing (main)

import Browser
import Browser.Navigation as Nav
import Dict exposing (Dict)
import Help
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick, onInput)
import Http
import Json.Decode as Decode
import Ports
import Process
import Protocol exposing (..)
import Set exposing (Set)
import Svg
import Svg.Attributes as SA
import Task
import Time
import Url exposing (Url)
import Url.Parser exposing (Parser)


type Route
    = Home
    | Me
    | NotFound


routeParser : Parser (Route -> a) a
routeParser =
    Url.Parser.oneOf
        [ Url.Parser.map Home Url.Parser.top
        , Url.Parser.map Me (Url.Parser.s "me")
        ]


routeFromUrl : Url -> Route
routeFromUrl url =
    Url.Parser.parse routeParser url
        |> Maybe.withDefault NotFound


type alias MeInfo =
    { name : String
    , authEnabled : Bool
    }


type MeStatus
    = MeLoading
    | MeLoaded MeInfo
    | MeFailed


{-| Inline form state for the "+ Add source" flow. Cleared when
the user submits or cancels. `nameEdited` flips to True on the
first keystroke in the name field so URL-driven hostname auto-fill
stops clobbering a name the user typed deliberately.
-}
type alias AddSourceForm =
    { name : String
    , url : String
    , nameEdited : Bool
    , error : Maybe String
    }


type alias Model =
    { key : Nav.Key
    , url : Url
    , route : Route
    , me : MeStatus
    , connected : Bool
    , patchParamMeta : List PatchParamMeta
    , library : Dict String (Dict String Float)
    , overrides : Dict String OverrideInfo
    , selectedPatch : Maybe String
    , heartbeats : List HeartbeatInfo
    , muted : Bool
    , masterVolume : Float
    , probeLog : List ProbeLogEntry
    , exportFormat : String
    , exportData : Maybe String
    , copyFeedback : Bool
    , debounces : Dict String Int
    , nextDebounce : Int
    , importText : String
    , importError : Maybe String
    , protocolError : Maybe String
    , sliderRanges : SliderRanges
    , renamingPatch : Maybe String
    , renameInput : String
    , configWritable : Bool
    , configPathResolved : Maybe String
    , headless : Bool
    , sources : List SourceInfo
    , addSourceForm : Maybe AddSourceForm
    , pendingRemoveSource : Set String
    , saveStatus : Maybe String
    , playOnChange : Set String
    , metricHistory : Dict Int (List Float)
    , timezone : Time.Zone
    , collapsedHeartbeats : Set Int
    , activeHelp : Maybe Help.Source
    }


type Msg
    = UrlRequested Browser.UrlRequest
    | UrlChanged Url
    | GotMe (Result Http.Error MeInfo)
    | WebSocketReceived String
    | SetPatchParam String String String
    | PatchParamDebounce String String Int Float
    | ToggleMute
    | ToggleRemotePlayback String
    | OpenAddSourceForm
    | CloseAddSourceForm
    | SetAddSourceName String
    | SetAddSourceUrl String
    | SubmitAddSource
    | RequestRemoveSource String
    | ConfirmRemoveSource String
    | CancelRemoveSource String
    | SetMasterVolume String
    | MasterVolDebounce Int Float
    | SetNoteSlider NoteSlider Int Int String
    | NoteSliderDebounce NoteSlider Int Int Int Float
    | SetNoteTransitionPatch Int Int Int String
    | SetNoteTransitionThreshold Int Int Int String
    | AddNoteTransitionState Int Int
    | RemoveNoteTransitionState Int Int Int
    | SwitchTransitionType Int Int String
    | SetSegmentStrategy Int Int Int String
    | SetSegmentIntensity Int Int Int String
    | AddNoteGradientPatch Int Int
    | RemoveNoteGradientPatch Int Int Int
    | AddNote Int
    | RemoveNote Int Int
    | OverrideHeartbeat Int String
    | OverrideDebounce Int Int Float
    | ClearOverride Int
    | CyclePlayback Int
    | TriggerHeartbeat Int
    | RevertAll
    | SelectPatch String
    | Export
    | DismissExport
    | SetExportFormat String
    | CopyExport
    | ClearCopyFeedback
    | SetImportText String
    | SubmitImport
    | SetHeartbeatSlider HeartbeatSlider Int String
    | HeartbeatSliderDebounce HeartbeatSlider Int Int Float
    | CreatePatch
    | CreateHeartbeat
    | CreateOverride String
    | StartRename String
    | SetRenameInput String
    | ConfirmRename String
    | CancelRename
    | ResetOverrideParam String String
    | SaveConfig
    | DismissSaveStatus
    | DismissProtocolError
    | TogglePlayOnChange String
    | GotTimezone Time.Zone
    | ToggleHeartbeatCollapse Int
    | SetHeartbeatName Int String
    | SetHeartbeatCommand Int String
    | SetHeartbeatResultMode Int String
    | AddTier Int
    | RemoveTier Int Int
    | SetTierThreshold Int Int String
    | SetTierLabel Int Int String
    | SetTierColor Int Int String
    | HelpClicked Help.Source
    | NoOp


main : Program () Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = UrlRequested
        , onUrlChange = UrlChanged
        }


init : () -> Url -> Nav.Key -> ( Model, Cmd Msg )
init _ url key =
    let
        route =
            routeFromUrl url
    in
    ( { key = key
      , url = url
      , route = route
      , me = MeLoading
      , connected = False
      , patchParamMeta = []
      , library = Dict.empty
      , overrides = Dict.empty
      , selectedPatch = Nothing
      , heartbeats = []
      , muted = False
      , masterVolume = 1.0
      , probeLog = []
      , exportFormat = "toml"
      , exportData = Nothing
      , copyFeedback = False
      , debounces = Dict.empty
      , nextDebounce = 0
      , importText = ""
      , importError = Nothing
      , protocolError = Nothing
      , sliderRanges = defaultSliderRanges
      , renamingPatch = Nothing
      , renameInput = ""
      , configWritable = False
      , configPathResolved = Nothing
      , headless = False
      , sources = []
      , addSourceForm = Nothing
      , pendingRemoveSource = Set.empty
      , saveStatus = Nothing
      , playOnChange = Set.empty
      , metricHistory = Dict.empty
      , timezone = Time.utc
      , collapsedHeartbeats = Set.empty
      , activeHelp = Nothing
      }
    , Cmd.batch [ cmdForRoute route, Task.perform GotTimezone Time.here ]
    )


defaultSliderRanges : SliderRanges
defaultSliderRanges =
    { masterVolume = { min = 0, max = 1, step = 0.01 }
    , cycleOffset = { min = 0, max = 60, step = 0.1 }
    , overrideMetric = { min = 0, max = 1, step = 0.01 }
    , noteVolume = { min = 0, max = 1, step = 0.01 }
    , noteOffset = { min = 0, max = 60, step = 0.1 }
    , segmentIntensity = { min = 0.1, max = 10, step = 0.1 }
    , discreteThreshold = { min = 0, max = 1, step = 0.01 }
    , stepPosition = { min = 0, max = 1, step = 0.01 }
    , crossfadeMs = { min = 0, max = 500, step = 1 }
    }


cmdForRoute : Route -> Cmd Msg
cmdForRoute route =
    case route of
        Me ->
            fetchMe

        _ ->
            Cmd.none


fetchMe : Cmd Msg
fetchMe =
    Http.get
        { url = "/me"
        , expect = Http.expectJson GotMe meDecoder
        }


meDecoder : Decode.Decoder MeInfo
meDecoder =
    Decode.map2 MeInfo
        (Decode.field "name" Decode.string)
        (Decode.field "auth_enabled" Decode.bool)



-- Debounce helper


debounceMs : Float
debounceMs =
    150


debounce : String -> Float -> Model -> (Int -> Float -> Msg) -> ( Model, Cmd Msg )
debounce key value model toMsg =
    let
        id =
            model.nextDebounce

        newDebounces =
            Dict.insert key id model.debounces
    in
    ( { model | debounces = newDebounces, nextDebounce = id + 1 }
    , Process.sleep debounceMs
        |> Task.perform (\_ -> toMsg id value)
    )


isCurrentDebounce : String -> Int -> Model -> Bool
isCurrentDebounce key id model =
    Dict.get key model.debounces == Just id



-- Update


update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        UrlRequested req ->
            case req of
                Browser.Internal url ->
                    ( model, Nav.pushUrl model.key (Url.toString url) )

                Browser.External href ->
                    ( model, Nav.load href )

        UrlChanged url ->
            let
                route =
                    routeFromUrl url
            in
            ( { model | url = url, route = route }
            , cmdForRoute route
            )

        GotMe result ->
            case result of
                Ok info ->
                    ( { model | me = MeLoaded info }, Cmd.none )

                Err _ ->
                    ( { model | me = MeFailed }, Cmd.none )

        WebSocketReceived raw ->
            case decodeServerMsg raw of
                Ok serverMsg ->
                    handleServerMsg serverMsg model

                Err err ->
                    ( { model | protocolError = Just err }
                    , Ports.consoleError err
                    )

        SetPatchParam patchName param rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        key =
                            "pp:" ++ patchName ++ ":" ++ param

                        updated =
                            { model
                                | library =
                                    Dict.update patchName
                                        (Maybe.map (Dict.insert param val))
                                        model.library
                            }
                    in
                    debounce key val updated (PatchParamDebounce patchName param)

                Nothing ->
                    ( model, Cmd.none )

        PatchParamDebounce patchName param id val ->
            let
                key =
                    "pp:" ++ patchName ++ ":" ++ param
            in
            if isCurrentDebounce key id model then
                -- Must be a single websocketSend, not a Cmd.batch
                -- of two messages, because Elm's Cmd.batch reverses
                -- port-send order: a `[setParam, play]` batch would
                -- actually send `play` first and cause the audition
                -- to play against the previous library state.  The
                -- backend's `set_patch_param_and_play` handler
                -- applies the change and plays atomically.
                ( model
                , if Set.member patchName model.playOnChange then
                    Ports.websocketSend
                        (encodeSetPatchParamAndPlay patchName param val)

                  else
                    Ports.websocketSend
                        (encodeSetPatchParam patchName param val)
                )

            else
                ( model, Cmd.none )

        ToggleMute ->
            ( model
            , Ports.websocketSend (encodeSetMuted (not model.muted))
            )

        ToggleRemotePlayback sourceName ->
            -- Optimistic flip: update the matching remote source's
            -- playbackEnabled in the model, then send the wire
            -- message.  The state snapshot the backend rebroadcasts
            -- after applying will overwrite the model anyway, but
            -- the optimistic update keeps the UI feeling
            -- responsive.
            let
                updatedSources =
                    List.map (toggleSourcePlayback sourceName) model.sources

                newEnabled =
                    findSourcePlayback sourceName updatedSources
            in
            ( { model | sources = updatedSources }
            , Ports.websocketSend
                (encodeSetRemotePlaybackEnabled sourceName newEnabled)
            )

        OpenAddSourceForm ->
            ( { model
                | addSourceForm =
                    Just
                        { name = ""
                        , url = ""
                        , nameEdited = False
                        , error = Nothing
                        }
              }
            , Cmd.none
            )

        CloseAddSourceForm ->
            ( { model | addSourceForm = Nothing }, Cmd.none )

        SetAddSourceName n ->
            ( { model
                | addSourceForm =
                    Maybe.map
                        (\f -> { f | name = n, nameEdited = True })
                        model.addSourceForm
              }
            , Cmd.none
            )

        SetAddSourceUrl u ->
            ( { model
                | addSourceForm =
                    Maybe.map
                        (\f ->
                            { f
                                | url = u
                                , name =
                                    if f.nameEdited then
                                        f.name

                                    else
                                        parseHostname u
                            }
                        )
                        model.addSourceForm
              }
            , Cmd.none
            )

        SubmitAddSource ->
            case model.addSourceForm of
                Just f ->
                    let
                        trimmedName =
                            String.trim f.name

                        trimmedUrl =
                            String.trim f.url
                    in
                    if String.isEmpty trimmedName || String.isEmpty trimmedUrl then
                        ( { model
                            | addSourceForm =
                                Just
                                    { f
                                        | error =
                                            Just
                                                "Name and URL are both required."
                                    }
                          }
                        , Cmd.none
                        )

                    else if trimmedName == "localhost" then
                        ( { model
                            | addSourceForm =
                                Just
                                    { f
                                        | error =
                                            Just
                                                "`localhost` is reserved for the Local Source."
                                    }
                          }
                        , Cmd.none
                        )

                    else
                        ( { model | addSourceForm = Nothing }
                        , Ports.websocketSend
                            (encodeAddRemoteSource trimmedName trimmedUrl)
                        )

                Nothing ->
                    ( model, Cmd.none )

        RequestRemoveSource name ->
            ( { model
                | pendingRemoveSource =
                    Set.insert name model.pendingRemoveSource
              }
            , Cmd.none
            )

        CancelRemoveSource name ->
            ( { model
                | pendingRemoveSource =
                    Set.remove name model.pendingRemoveSource
              }
            , Cmd.none
            )

        ConfirmRemoveSource name ->
            ( { model
                | pendingRemoveSource =
                    Set.remove name model.pendingRemoveSource
              }
            , Ports.websocketSend (encodeRemoveRemoteSource name)
            )

        SetMasterVolume rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    debounce "master_vol" val { model | masterVolume = val } MasterVolDebounce

                Nothing ->
                    ( model, Cmd.none )

        MasterVolDebounce id val ->
            if isCurrentDebounce "master_vol" id model then
                ( model
                , Ports.websocketSend (encodeSetMasterVolume val)
                )

            else
                ( model, Cmd.none )

        SetNoteSlider slider hbIdx noteIdx rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        key =
                            noteSliderKey slider
                                ++ String.fromInt hbIdx
                                ++ ":"
                                ++ String.fromInt noteIdx

                        updated =
                            { model
                                | heartbeats =
                                    updateAt hbIdx
                                        (\hb ->
                                            { hb
                                                | notes =
                                                    updateAt noteIdx
                                                        (setNoteSliderValue slider val)
                                                        hb.notes
                                            }
                                        )
                                        model.heartbeats
                            }
                    in
                    debounce key val updated (NoteSliderDebounce slider hbIdx noteIdx)

                Nothing ->
                    ( model, Cmd.none )

        NoteSliderDebounce slider hbIdx noteIdx id val ->
            let
                key =
                    noteSliderKey slider
                        ++ String.fromInt hbIdx
                        ++ ":"
                        ++ String.fromInt noteIdx
            in
            if isCurrentDebounce key id model then
                ( model
                , Ports.websocketSend (encodeNoteSlider slider hbIdx noteIdx val)
                )

            else
                ( model, Cmd.none )

        SetNoteTransitionPatch hbIdx noteIdx stateIdx patchName ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (updateTransitionPatch stateIdx patchName)
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        SetNoteTransitionThreshold hbIdx noteIdx stateIdx rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        newHeartbeats =
                            updateNoteTransition hbIdx
                                noteIdx
                                (updateTransitionThreshold stateIdx val)
                                model.heartbeats

                        newTrans =
                            getNoteTransition hbIdx noteIdx newHeartbeats
                    in
                    ( { model | heartbeats = newHeartbeats }
                    , newTrans
                        |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                        |> Maybe.withDefault Cmd.none
                    )

                Nothing ->
                    ( model, Cmd.none )

        AddNoteTransitionState hbIdx noteIdx ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        addDiscreteState
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        RemoveNoteTransitionState hbIdx noteIdx stateIdx ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (removeDiscreteState stateIdx)
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        SwitchTransitionType hbIdx noteIdx typeName ->
            let
                newTrans =
                    switchTransitionType model typeName

                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (\_ -> newTrans)
                        model.heartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx newTrans)
            )

        SetSegmentStrategy hbIdx noteIdx segIdx rawName ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (changeStrategy segIdx rawName)
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        SetSegmentIntensity hbIdx noteIdx segIdx rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        newHeartbeats =
                            updateNoteTransition hbIdx
                                noteIdx
                                (changeIntensity segIdx val)
                                model.heartbeats

                        newTrans =
                            getNoteTransition hbIdx noteIdx newHeartbeats
                    in
                    ( { model | heartbeats = newHeartbeats }
                    , newTrans
                        |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                        |> Maybe.withDefault Cmd.none
                    )

                Nothing ->
                    ( model, Cmd.none )

        AddNoteGradientPatch hbIdx noteIdx ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (addGradientPatch model)
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        RemoveNoteGradientPatch hbIdx noteIdx patchIdx ->
            let
                newHeartbeats =
                    updateNoteTransition hbIdx
                        noteIdx
                        (removeGradientPatch patchIdx)
                        model.heartbeats

                newTrans =
                    getNoteTransition hbIdx noteIdx newHeartbeats
            in
            ( { model | heartbeats = newHeartbeats }
            , newTrans
                |> Maybe.map (\t -> Ports.websocketSend (encodeSetNoteTransition hbIdx noteIdx t))
                |> Maybe.withDefault Cmd.none
            )

        AddNote hbIdx ->
            ( model
            , Ports.websocketSend (encodeAddNote hbIdx)
            )

        RemoveNote hbIdx noteIdx ->
            ( model
            , Ports.websocketSend (encodeRemoveNote hbIdx noteIdx)
            )

        OverrideHeartbeat index rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        key =
                            "override:" ++ String.fromInt index

                        updated =
                            { model
                                | heartbeats =
                                    updateAt index
                                        (\hb -> { hb | metric = val, overridden = True })
                                        model.heartbeats
                            }
                    in
                    debounce key val updated (OverrideDebounce index)

                Nothing ->
                    ( model, Cmd.none )

        OverrideDebounce index id val ->
            let
                key =
                    "override:" ++ String.fromInt index
            in
            if isCurrentDebounce key id model then
                ( model
                , Ports.websocketSend (encodeOverrideHeartbeat index val)
                )

            else
                ( model, Cmd.none )

        ClearOverride index ->
            ( model
            , Ports.websocketSend (encodeClearOverride index)
            )

        CyclePlayback index ->
            let
                hb =
                    List.drop index model.heartbeats
                        |> List.head

                nextMode current =
                    case current of
                        "clock" ->
                            "loop"

                        "loop" ->
                            "continuous"

                        _ ->
                            "clock"
            in
            case hb of
                Just h ->
                    let
                        next =
                            nextMode h.playback
                    in
                    ( { model
                        | heartbeats =
                            updateAt index
                                (\hbi -> { hbi | playback = next })
                                model.heartbeats
                      }
                    , Ports.websocketSend (encodeSetPlayback index next)
                    )

                Nothing ->
                    ( model, Cmd.none )

        TriggerHeartbeat index ->
            ( model, Ports.websocketSend (encodeTriggerHeartbeat index) )

        RevertAll ->
            ( model, Ports.websocketSend encodeRevertAll )

        SelectPatch name ->
            ( { model | selectedPatch = Just name }, Cmd.none )

        Export ->
            ( model, Ports.websocketSend (encodeExportConfig model.exportFormat) )

        DismissExport ->
            ( { model | exportData = Nothing, exportFormat = "toml", copyFeedback = False }, Cmd.none )

        SetExportFormat fmt ->
            ( { model | exportFormat = fmt }, Cmd.none )

        CopyExport ->
            case model.exportData of
                Just txt ->
                    ( { model | copyFeedback = True }
                    , Cmd.batch
                        [ Ports.copyToClipboard txt
                        , Process.sleep 1500
                            |> Task.perform (\_ -> ClearCopyFeedback)
                        ]
                    )

                Nothing ->
                    ( model, Cmd.none )

        ClearCopyFeedback ->
            ( { model | copyFeedback = False }, Cmd.none )

        SetImportText txt ->
            ( { model | importText = txt, importError = Nothing }, Cmd.none )

        SubmitImport ->
            if String.isEmpty (String.trim model.importText) then
                ( model, Cmd.none )

            else
                ( { model | importError = Nothing }
                , Ports.websocketSend (encodeImportConfig model.importText)
                )

        SetHeartbeatSlider slider index rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        key =
                            heartbeatSliderKey slider ++ String.fromInt index

                        updated =
                            { model
                                | heartbeats =
                                    updateAt index
                                        (setHeartbeatSliderValue slider val)
                                        model.heartbeats
                            }
                    in
                    debounce key val updated (HeartbeatSliderDebounce slider index)

                Nothing ->
                    ( model, Cmd.none )

        HeartbeatSliderDebounce slider index id val ->
            let
                key =
                    heartbeatSliderKey slider ++ String.fromInt index
            in
            if isCurrentDebounce key id model then
                ( model
                , Ports.websocketSend (encodeHeartbeatSlider slider index val)
                )

            else
                ( model, Cmd.none )

        CreatePatch ->
            let
                candidate =
                    "new-patch"

                name =
                    if Dict.member candidate model.library then
                        findUniqueName candidate 2 model.library

                    else
                        candidate
            in
            ( { model | selectedPatch = Just name }
            , Ports.websocketSend (encodeCreatePatch name)
            )

        CreateHeartbeat ->
            let
                names =
                    List.map .name model.heartbeats

                candidate =
                    "new-heartbeat"

                name =
                    if List.member candidate names then
                        findUniqueHeartbeatName candidate 2 names

                    else
                        candidate
            in
            ( model
            , Ports.websocketSend (encodeCreateHeartbeat name)
            )

        CreateOverride base ->
            let
                candidate =
                    base ++ "-override"

                name =
                    if Dict.member candidate model.library then
                        findUniqueName candidate 2 model.library

                    else
                        candidate
            in
            ( model
            , Ports.websocketSend (encodeCreateOverride base name)
            )

        StartRename name ->
            ( { model | renamingPatch = Just name, renameInput = name }
            , Cmd.none
            )

        SetRenameInput txt ->
            ( { model | renameInput = txt }, Cmd.none )

        ConfirmRename oldName ->
            let
                newName =
                    String.trim model.renameInput

                cmd =
                    if not (String.isEmpty newName) && newName /= oldName then
                        Ports.websocketSend (encodeRenamePatch oldName newName)

                    else
                        Cmd.none

                updatedSelection =
                    if model.selectedPatch == Just oldName && newName /= oldName && not (String.isEmpty newName) then
                        Just newName

                    else
                        model.selectedPatch
            in
            ( { model
                | renamingPatch = Nothing
                , renameInput = ""
                , selectedPatch = updatedSelection
              }
            , cmd
            )

        CancelRename ->
            ( { model | renamingPatch = Nothing, renameInput = "" }
            , Cmd.none
            )

        ResetOverrideParam patchName param ->
            ( model
            , Ports.websocketSend (encodeResetOverrideParam patchName param)
            )

        SaveConfig ->
            ( model, Ports.websocketSend encodeSaveConfig )

        DismissSaveStatus ->
            ( { model | saveStatus = Nothing }, Cmd.none )

        DismissProtocolError ->
            ( { model | protocolError = Nothing }, Cmd.none )

        TogglePlayOnChange patchName ->
            ( { model
                | playOnChange =
                    if Set.member patchName model.playOnChange then
                        Set.remove patchName model.playOnChange

                    else
                        Set.insert patchName model.playOnChange
              }
            , Cmd.none
            )

        GotTimezone zone ->
            ( { model | timezone = zone }, Cmd.none )

        ToggleHeartbeatCollapse index ->
            ( { model
                | collapsedHeartbeats =
                    if Set.member index model.collapsedHeartbeats then
                        Set.remove index model.collapsedHeartbeats

                    else
                        Set.insert index model.collapsedHeartbeats
              }
            , Cmd.none
            )

        SetHeartbeatName hbIdx val ->
            ( { model
                | heartbeats =
                    updateAt hbIdx (\hb -> { hb | name = val }) model.heartbeats
              }
            , Ports.websocketSend (encodeSetHeartbeatString "set_heartbeat_name" hbIdx val)
            )

        SetHeartbeatCommand hbIdx val ->
            ( { model
                | heartbeats =
                    updateAt hbIdx (\hb -> { hb | command = val }) model.heartbeats
              }
            , Ports.websocketSend (encodeSetHeartbeatString "set_heartbeat_command" hbIdx val)
            )

        SetHeartbeatResultMode hbIdx val ->
            ( { model
                | heartbeats =
                    updateAt hbIdx (\hb -> { hb | resultMode = val }) model.heartbeats
              }
            , Ports.websocketSend (encodeSetHeartbeatString "set_result_mode" hbIdx val)
            )

        AddTier hbIdx ->
            let
                newTier =
                    { threshold = 1.01, label = "new", color = "#888888" }

                newHeartbeats =
                    updateAt hbIdx
                        (\hb -> { hb | tiers = hb.tiers ++ [ newTier ] })
                        model.heartbeats

                tiers =
                    getAt hbIdx newHeartbeats
                        |> Maybe.map .tiers
                        |> Maybe.withDefault []
            in
            ( { model | heartbeats = newHeartbeats }
            , Ports.websocketSend (encodeSetTiers hbIdx tiers)
            )

        RemoveTier hbIdx tierIdx ->
            let
                newHeartbeats =
                    updateAt hbIdx
                        (\hb -> { hb | tiers = removeAt tierIdx hb.tiers })
                        model.heartbeats

                tiers =
                    getAt hbIdx newHeartbeats
                        |> Maybe.map .tiers
                        |> Maybe.withDefault []
            in
            ( { model | heartbeats = newHeartbeats }
            , Ports.websocketSend (encodeSetTiers hbIdx tiers)
            )

        SetTierThreshold hbIdx tierIdx raw ->
            case String.toFloat raw of
                Just val ->
                    let
                        newHeartbeats =
                            updateAt hbIdx
                                (\hb ->
                                    { hb
                                        | tiers =
                                            updateAt tierIdx
                                                (\t -> { t | threshold = val })
                                                hb.tiers
                                    }
                                )
                                model.heartbeats

                        tiers =
                            getAt hbIdx newHeartbeats
                                |> Maybe.map .tiers
                                |> Maybe.withDefault []
                    in
                    ( { model | heartbeats = newHeartbeats }
                    , Ports.websocketSend (encodeSetTiers hbIdx tiers)
                    )

                Nothing ->
                    ( model, Cmd.none )

        SetTierLabel hbIdx tierIdx val ->
            let
                newHeartbeats =
                    updateAt hbIdx
                        (\hb ->
                            { hb
                                | tiers =
                                    updateAt tierIdx
                                        (\t -> { t | label = val })
                                        hb.tiers
                            }
                        )
                        model.heartbeats

                tiers =
                    getAt hbIdx newHeartbeats
                        |> Maybe.map .tiers
                        |> Maybe.withDefault []
            in
            ( { model | heartbeats = newHeartbeats }
            , Ports.websocketSend (encodeSetTiers hbIdx tiers)
            )

        SetTierColor hbIdx tierIdx val ->
            let
                newHeartbeats =
                    updateAt hbIdx
                        (\hb ->
                            { hb
                                | tiers =
                                    updateAt tierIdx
                                        (\t -> { t | color = val })
                                        hb.tiers
                            }
                        )
                        model.heartbeats

                tiers =
                    getAt hbIdx newHeartbeats
                        |> Maybe.map .tiers
                        |> Maybe.withDefault []
            in
            ( { model | heartbeats = newHeartbeats }
            , Ports.websocketSend (encodeSetTiers hbIdx tiers)
            )

        HelpClicked key ->
            ( { model | activeHelp = Just key }, Cmd.none )

        NoOp ->
            ( model, Cmd.none )


handleServerMsg : ServerMsg -> Model -> ( Model, Cmd Msg )
handleServerMsg msg model =
    case msg of
        StateMsg state ->
            ( { model
                | patchParamMeta = state.patchParams
                , library = state.library
                , overrides = state.overrides
                , muted = state.muted
                , masterVolume = state.masterVolume
                , heartbeats = state.heartbeats
                , sliderRanges = state.sliderRanges
                , configWritable = state.configWritable
                , configPathResolved = state.configPathResolved
                , headless = state.headless
                , sources = state.sources
                , selectedPatch =
                    case model.selectedPatch of
                        Nothing ->
                            Dict.keys state.library |> List.head

                        Just name ->
                            if Dict.member name state.library then
                                Just name

                            else
                                Dict.keys state.library |> List.head
              }
            , Cmd.none
            )

        PatchParamChanged patchName param value ->
            ( { model
                | library =
                    Dict.update patchName
                        (Maybe.map (Dict.insert param value))
                        model.library
              }
            , Cmd.none
            )

        MuteChanged muted ->
            ( { model | muted = muted }, Cmd.none )

        VolumeChanged layer _ volume ->
            case layer of
                "master" ->
                    ( { model | masterVolume = volume }, Cmd.none )

                _ ->
                    ( model, Cmd.none )

        MetricChanged index value ->
            let
                existing =
                    Dict.get index model.metricHistory
                        |> Maybe.withDefault []

                updated =
                    List.take 99 (existing ++ [ value ])
            in
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb -> { hb | metric = value })
                        model.heartbeats
                , metricHistory = Dict.insert index updated model.metricHistory
              }
            , Cmd.none
            )

        OverrideChanged index maybeValue overridden ->
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb ->
                            { hb
                                | overridden = overridden
                                , metric =
                                    Maybe.withDefault hb.metric maybeValue
                            }
                        )
                        model.heartbeats
              }
            , Cmd.none
            )

        PlaybackChanged index value ->
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb -> { hb | playback = value })
                        model.heartbeats
              }
            , Cmd.none
            )

        LibraryChanged lib ->
            ( { model | library = lib }, Cmd.none )

        OverridesChanged ovr ->
            ( { model | overrides = ovr }, Cmd.none )

        HeartbeatSliderChanged slider index value ->
            ( { model
                | heartbeats =
                    updateAt index
                        (setHeartbeatSliderValue slider value)
                        model.heartbeats
              }
            , Cmd.none
            )

        HeartbeatStringChanged field index value ->
            ( { model
                | heartbeats =
                    updateAt index
                        (setHeartbeatStringValue field value)
                        model.heartbeats
              }
            , Cmd.none
            )

        NoteSliderChanged slider hbIdx noteIdx value ->
            ( { model
                | heartbeats =
                    updateAt hbIdx
                        (\hb ->
                            { hb
                                | notes =
                                    updateAt noteIdx
                                        (setNoteSliderValue slider value)
                                        hb.notes
                            }
                        )
                        model.heartbeats
              }
            , Cmd.none
            )

        NoteTransitionChanged hbIdx noteIdx trans ->
            ( { model
                | heartbeats =
                    updateAt hbIdx
                        (\hb ->
                            { hb
                                | notes =
                                    updateAt noteIdx
                                        (\n -> { n | transition = trans })
                                        hb.notes
                            }
                        )
                        model.heartbeats
              }
            , Cmd.none
            )

        NotesChanged hbIdx notes ->
            ( { model
                | heartbeats =
                    updateAt hbIdx
                        (\hb -> { hb | notes = notes })
                        model.heartbeats
              }
            , Cmd.none
            )

        TiersChanged hbIdx tiers ->
            ( { model
                | heartbeats =
                    updateAt hbIdx
                        (\hb -> { hb | tiers = tiers })
                        model.heartbeats
              }
            , Cmd.none
            )

        ProbeLog entry ->
            ( { model | probeLog = entry :: List.take 99 model.probeLog }
            , Cmd.none
            )

        ConfigExport tomlText ->
            ( { model | exportData = Just tomlText }, Cmd.none )

        ImportError err ->
            ( { model | importError = Just err }, Cmd.none )

        ConfigSaved ->
            ( { model | saveStatus = Just "Saved" }
            , Process.sleep 2000
                |> Task.perform (\_ -> DismissSaveStatus)
            )

        SaveError err ->
            ( { model | saveStatus = Just err }, Cmd.none )

        WsConnected ->
            ( { model | connected = True }
            , Ports.websocketSend encodeGetState
            )

        WsDisconnected ->
            ( { model | connected = False }, Cmd.none )


updateAt : Int -> (a -> a) -> List a -> List a
updateAt index fn list =
    List.indexedMap
        (\i item ->
            if i == index then
                fn item

            else
                item
        )
        list


heartbeatSliderKey : HeartbeatSlider -> String
heartbeatSliderKey slider =
    case slider of
        CycleOffset ->
            "hb_offset:"

        CrossfadeMs ->
            "hb_crossfade:"

        PollInterval ->
            "hb_poll:"

        CycleSecs ->
            "hb_cycle:"

        PhraseGap ->
            "hb_phrase:"

        RepeatRate ->
            "hb_repeat:"


setHeartbeatSliderValue : HeartbeatSlider -> Float -> HeartbeatInfo -> HeartbeatInfo
setHeartbeatSliderValue slider val hb =
    case slider of
        CycleOffset ->
            { hb | cycleOffsetSecs = val }

        CrossfadeMs ->
            { hb | crossfadeMs = val }

        PollInterval ->
            { hb | pollIntervalSecs = val }

        CycleSecs ->
            { hb | cycleSecs = val }

        PhraseGap ->
            { hb | phraseGap = val }

        RepeatRate ->
            { hb | repeatRate = val }


setHeartbeatStringValue : String -> String -> HeartbeatInfo -> HeartbeatInfo
setHeartbeatStringValue field val hb =
    case field of
        "name" ->
            { hb | name = val }

        "command" ->
            { hb | command = val }

        "result_mode" ->
            { hb | resultMode = val }

        _ ->
            hb


noteSliderKey : NoteSlider -> String
noteSliderKey slider =
    case slider of
        NoteVolume ->
            "nv:"

        NoteOffset ->
            "no:"


setNoteSliderValue : NoteSlider -> Float -> NoteInfo -> NoteInfo
setNoteSliderValue slider val note =
    case slider of
        NoteVolume ->
            { note | volume = val }

        NoteOffset ->
            { note | offset = val }


onEnter : Msg -> Html.Attribute Msg
onEnter msg =
    Html.Events.on "keydown"
        (Decode.field "key" Decode.string
            |> Decode.andThen
                (\key ->
                    if key == "Enter" then
                        Decode.succeed msg

                    else
                        Decode.fail "not enter"
                )
        )


onEsc : Msg -> Html.Attribute Msg
onEsc msg =
    Html.Events.on "keydown"
        (Decode.field "key" Decode.string
            |> Decode.andThen
                (\key ->
                    if key == "Escape" then
                        Decode.succeed msg

                    else
                        Decode.fail "not escape"
                )
        )


findUniqueName : String -> Int -> Dict String a -> String
findUniqueName base n library =
    let
        candidate =
            base ++ "-" ++ String.fromInt n
    in
    if Dict.member candidate library then
        findUniqueName base (n + 1) library

    else
        candidate


findUniqueHeartbeatName : String -> Int -> List String -> String
findUniqueHeartbeatName base n names =
    let
        candidate =
            base ++ "-" ++ String.fromInt n
    in
    if List.member candidate names then
        findUniqueHeartbeatName base (n + 1) names

    else
        candidate


unique : List comparable -> List comparable
unique list =
    List.foldl
        (\item ( seen, acc ) ->
            if List.member item seen then
                ( seen, acc )

            else
                ( item :: seen, acc ++ [ item ] )
        )
        ( [], [] )
        list
        |> Tuple.second



-- Note transition editing helpers


getAt : Int -> List a -> Maybe a
getAt index list =
    List.drop index list |> List.head


removeAt : Int -> List a -> List a
removeAt index list =
    List.take index list ++ List.drop (index + 1) list


updateNoteTransition : Int -> Int -> (TransitionInfo -> TransitionInfo) -> List HeartbeatInfo -> List HeartbeatInfo
updateNoteTransition hbIdx noteIdx fn heartbeats =
    updateAt hbIdx
        (\hb ->
            { hb
                | notes =
                    updateAt noteIdx
                        (\n -> { n | transition = fn n.transition })
                        hb.notes
            }
        )
        heartbeats


getNoteTransition : Int -> Int -> List HeartbeatInfo -> Maybe TransitionInfo
getNoteTransition hbIdx noteIdx heartbeats =
    getAt hbIdx heartbeats
        |> Maybe.andThen (\hb -> getAt noteIdx hb.notes)
        |> Maybe.map .transition


updateTransitionPatch : Int -> String -> TransitionInfo -> TransitionInfo
updateTransitionPatch stateIdx patchName trans =
    case trans of
        Discrete states ->
            Discrete
                (List.indexedMap
                    (\i s ->
                        if i == stateIdx then
                            { s | patch = patchName }

                        else
                            s
                    )
                    states
                )

        Gradient info ->
            Gradient
                { info
                    | patches =
                        List.indexedMap
                            (\i p ->
                                if i == stateIdx then
                                    patchName

                                else
                                    p
                            )
                            info.patches
                }


updateTransitionThreshold : Int -> Float -> TransitionInfo -> TransitionInfo
updateTransitionThreshold stateIdx val trans =
    case trans of
        Discrete states ->
            Discrete
                (List.indexedMap
                    (\i s ->
                        if i == stateIdx then
                            { s | threshold = val }

                        else
                            s
                    )
                    states
                )

        Gradient info ->
            Gradient info


addDiscreteState : TransitionInfo -> TransitionInfo
addDiscreteState trans =
    case trans of
        Discrete states ->
            Discrete (states ++ [ { threshold = 1.0, patch = "sine" } ])

        Gradient info ->
            Gradient info


removeDiscreteState : Int -> TransitionInfo -> TransitionInfo
removeDiscreteState stateIdx trans =
    case trans of
        Discrete states ->
            if List.length states > 1 then
                Discrete
                    (List.indexedMap Tuple.pair states
                        |> List.filterMap
                            (\( i, s ) ->
                                if i == stateIdx then
                                    Nothing

                                else
                                    Just s
                            )
                    )

            else
                Discrete states

        Gradient info ->
            Gradient info


switchTransitionType : Model -> String -> TransitionInfo
switchTransitionType model typeName =
    let
        defaultPatch =
            Dict.keys model.library
                |> List.head
                |> Maybe.withDefault "sine"
    in
    case typeName of
        "gradient" ->
            Gradient { patches = [ defaultPatch, defaultPatch ], segments = [ Linear 2.0 ] }

        _ ->
            Discrete [ { threshold = 1.0, patch = defaultPatch } ]


changeStrategy : Int -> String -> TransitionInfo -> TransitionInfo
changeStrategy segIdx rawName trans =
    case trans of
        Gradient info ->
            let
                newSegments =
                    List.indexedMap
                        (\i seg ->
                            if i == segIdx then
                                let
                                    oldIntensity =
                                        strategyIntensity seg
                                in
                                case rawName of
                                    "ease-in" ->
                                        EaseIn oldIntensity

                                    "ease-out" ->
                                        EaseOut oldIntensity

                                    "ease-in-out" ->
                                        EaseInOut oldIntensity

                                    "step" ->
                                        Step 0.5

                                    _ ->
                                        Linear oldIntensity

                            else
                                seg
                        )
                        info.segments
            in
            Gradient { info | segments = newSegments }

        Discrete _ ->
            trans


changeIntensity : Int -> Float -> TransitionInfo -> TransitionInfo
changeIntensity segIdx val trans =
    case trans of
        Gradient info ->
            let
                newSegments =
                    List.indexedMap
                        (\i seg ->
                            if i == segIdx then
                                setStrategyIntensity val seg

                            else
                                seg
                        )
                        info.segments
            in
            Gradient { info | segments = newSegments }

        Discrete _ ->
            trans


strategyIntensity : LerpStrategy -> Float
strategyIntensity strat =
    case strat of
        Linear i ->
            i

        EaseIn i ->
            i

        EaseOut i ->
            i

        EaseInOut i ->
            i

        Step i ->
            i


setStrategyIntensity : Float -> LerpStrategy -> LerpStrategy
setStrategyIntensity val strat =
    case strat of
        Linear _ ->
            Linear val

        EaseIn _ ->
            EaseIn val

        EaseOut _ ->
            EaseOut val

        EaseInOut _ ->
            EaseInOut val

        Step _ ->
            Step val


strategyName : LerpStrategy -> String
strategyName strat =
    case strat of
        Linear _ ->
            "linear"

        EaseIn _ ->
            "ease-in"

        EaseOut _ ->
            "ease-out"

        EaseInOut _ ->
            "ease-in-out"

        Step _ ->
            "step"


applyStrategy : LerpStrategy -> Float -> Float
applyStrategy strat t =
    let
        tc =
            clamp 0 1 t
    in
    case strat of
        Linear _ ->
            tc

        EaseIn intensity ->
            tc ^ intensity

        EaseOut intensity ->
            1 - (1 - tc) ^ intensity

        EaseInOut intensity ->
            if tc < 0.5 then
                0.5 * (2 * tc) ^ intensity

            else
                1 - 0.5 * (2 - 2 * tc) ^ intensity

        Step intensity ->
            if tc < intensity then
                0

            else
                1


syncSegments : List LerpStrategy -> Int -> List LerpStrategy
syncSegments segments patchCount =
    let
        needed =
            Basics.max 0 (patchCount - 1)

        current =
            List.length segments
    in
    if current >= needed then
        List.take needed segments

    else
        segments ++ List.repeat (needed - current) (Linear 2.0)


addGradientPatch : Model -> TransitionInfo -> TransitionInfo
addGradientPatch model trans =
    case trans of
        Gradient info ->
            let
                defaultPatch =
                    Dict.keys model.library
                        |> List.head
                        |> Maybe.withDefault "sine"

                newPatches =
                    info.patches ++ [ defaultPatch ]
            in
            Gradient
                { patches = newPatches
                , segments = syncSegments info.segments (List.length newPatches)
                }

        Discrete states ->
            Discrete states


removeGradientPatch : Int -> TransitionInfo -> TransitionInfo
removeGradientPatch patchIdx trans =
    case trans of
        Gradient info ->
            if List.length info.patches > 1 then
                let
                    newPatches =
                        List.indexedMap Tuple.pair info.patches
                            |> List.filterMap
                                (\( i, p ) ->
                                    if i == patchIdx then
                                        Nothing

                                    else
                                        Just p
                                )

                    segIdxToRemove =
                        Basics.max 0 (patchIdx - 1)

                    newSegments =
                        List.indexedMap Tuple.pair info.segments
                            |> List.filterMap
                                (\( i, s ) ->
                                    if i == segIdxToRemove then
                                        Nothing

                                    else
                                        Just s
                                )
                in
                Gradient
                    { patches = newPatches
                    , segments = syncSegments newSegments (List.length newPatches)
                    }

            else
                Gradient info

        Discrete states ->
            Discrete states



-- Subscriptions


subscriptions : Model -> Sub Msg
subscriptions _ =
    Ports.websocketReceive WebSocketReceived



-- View


view : Model -> Browser.Document Msg
view model =
    { title = "sonify-health"
    , body =
        [ viewNavbar model
        , case model.route of
            Home ->
                viewHome model

            Me ->
                viewMe model

            NotFound ->
                div [ class "container" ] [ text "Not found" ]
        , viewHelpPanel model
        ]
    }


{-| Bottom-fixed panel that explains whichever label the user
most recently clicked. Stays visible until another label is
clicked; the "no selection" placeholder is shown on first load.
See `Help.elm` for the content registry.
-}
viewHelpPanel : Model -> Html Msg
viewHelpPanel model =
    let
        body =
            case model.activeHelp of
                Nothing ->
                    [ span [ class "help-panel-placeholder" ]
                        [ text "Click a dotted-underline label anywhere in the UI for a short explanation." ]
                    ]

                Just (Help.Registered key) ->
                    case Help.lookup key of
                        Just txt ->
                            renderHelpText txt

                        Nothing ->
                            [ span [ class "help-panel-placeholder" ]
                                [ text ("No help entry for \"" ++ key ++ "\" yet.") ]
                            ]

                Just (Help.Literal txt) ->
                    renderHelpText txt
    in
    aside [ class "help-panel" ]
        [ div [ class "help-panel-body" ]
            (div [ class "help-panel-title" ] [ text "Help" ] :: body)
        ]


{-| Parse backtick-delimited spans inside help text into inline
`<code>` elements, leaving the rest as plain text. Backticks are
used to reference other labels/fields in the UI (e.g. the
`attack_ms` reference inside the `duration` description) so they
visually stand out as identifiers.

Splitting on a single backtick and alternating (even index → plain
text, odd index → `<code>`) is correct regardless of how many
backticked spans appear, as long as they're balanced — which the
authoring convention enforces. Unbalanced backticks (one
trailing), if anyone introduces them, degrade gracefully: the
trailing content just gets styled as code to end-of-string.

See the TODO in tasks.org about making these spans clickable so
they navigate to the referenced help entry.

-}
renderHelpText : String -> List (Html Msg)
renderHelpText txt =
    txt
        |> String.split "`"
        |> List.indexedMap
            (\i part ->
                if modBy 2 i == 1 then
                    code [] [ text part ]

                else
                    text part
            )


viewNavbar : Model -> Html Msg
viewNavbar model =
    nav [ class "navbar" ]
        [ a [ href "/", class "nav-brand" ] [ text "sonify-health" ]
        , div [ class "nav-links" ]
            [ a [ href "/" ] [ text "Dashboard" ]
            , a [ href "/me" ] [ text "Me" ]
            ]
        , div [ class "nav-status" ]
            [ span
                [ class
                    (if model.connected then
                        "status-dot connected"

                     else
                        "status-dot disconnected"
                    )
                ]
                []
            , text
                (if model.connected then
                    "Connected"

                 else
                    "Disconnected"
                )
            ]
        ]


viewHome : Model -> Html Msg
viewHome model =
    let
        remotePanels =
            model.sources
                |> List.filter
                    (\s ->
                        case s.kind of
                            Remote _ ->
                                True

                            Local ->
                                False
                    )
                |> List.map (viewRemoteSourcePanel model.pendingRemoveSource)
    in
    div [ class "app-layout" ]
        [ viewToolbar model
        , div [ class "split-panel" ]
            [ div [ class "panel-left" ]
                -- The probe log is the one section whose height
                -- grows with traffic (capped to 200 px by
                -- `.log-container { max-height }`, but still 0 →
                -- 200 px between an empty log and a full one).
                -- Putting it last in the column keeps everything
                -- above it stationary as entries arrive instead of
                -- shoving the import/add-source/remote panels down
                -- by a fluctuating amount.
                ([ viewSourcePanel "localhost" model.headless [ viewHeartbeats model ]
                 , viewImport model
                 ]
                    ++ remotePanels
                    ++ [ viewAddSourceButton model.addSourceForm
                       , viewProbeLog model
                       ]
                )
            , div [ class "panel-right" ]
                [ viewPatchList model
                , viewPatchEditor model
                ]
            ]
        , viewExportInline model
        , viewProtocolError model
        ]


viewToolbar : Model -> Html Msg
viewToolbar model =
    div [ class "toolbar" ]
        [ button
            [ class
                (if model.muted then
                    "btn btn-danger"

                 else
                    "btn"
                )
            , onClick ToggleMute
            ]
            [ text
                (if model.muted then
                    "Unmute"

                 else
                    "Mute"
                )
            ]
        , viewSlider "Master" (Just "Global volume multiplier applied to all heartbeats.") model.sliderRanges.masterVolume.min model.sliderRanges.masterVolume.max model.sliderRanges.masterVolume.step model.masterVolume SetMasterVolume
        , button [ class "btn", onClick RevertAll ]
            [ text "Revert" ]
        , button
            (if model.configWritable then
                [ class "btn", onClick SaveConfig ]

             else
                [ class "btn"
                , Html.Attributes.disabled True
                , title
                    (case model.configPathResolved of
                        Just p ->
                            "Save disabled: " ++ p ++ " is not writable"

                        Nothing ->
                            "Save disabled: no config file"
                    )
                ]
            )
            [ text "Save" ]
        , case model.saveStatus of
            Just status ->
                span
                    [ class
                        (if status == "Saved" then
                            "save-status"

                         else
                            "save-status save-status-error"
                        )
                    ]
                    [ text status ]

            Nothing ->
                text ""
        , select
            [ class "export-format-select"
            , onInput SetExportFormat
            ]
            [ option [ value "toml", selected (model.exportFormat == "toml") ] [ text "TOML" ]
            , option [ value "json", selected (model.exportFormat == "json") ] [ text "JSON" ]
            , option [ value "nix", selected (model.exportFormat == "nix") ] [ text "Nix" ]
            ]
        , button [ class "btn", onClick Export ]
            [ text "Export" ]
        ]



-- Shared slider + number input component.


viewSlider : String -> Maybe String -> Float -> Float -> Float -> Float -> (String -> Msg) -> Html Msg
viewSlider name description min_ max_ step_ val toMsg =
    let
        -- If a description is present, render the label as a
        -- `help-label` button that pipes the text into the bottom
        -- help panel on click.  No description ⇒ plain `span`
        -- (non-interactive, no affordance).
        labelNode =
            case description of
                Just desc ->
                    button
                        [ class "help-label"
                        , onClick (HelpClicked (Help.Literal desc))
                        ]
                        [ text (name ++ " ") ]

                Nothing ->
                    span [] [ text (name ++ " ") ]
    in
    label [ class "slider-row" ]
        [ labelNode
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat min_)
            , Html.Attributes.max (String.fromFloat max_)
            , step (String.fromFloat step_)
            , value (String.fromFloat val)
            , onInput toMsg
            ]
            []
        , input
            [ type_ "number"
            , class "num-input"
            , Html.Attributes.min (String.fromFloat min_)
            , Html.Attributes.max (String.fromFloat max_)
            , step (String.fromFloat step_)
            , value (String.fromFloat val)
            , onInput toMsg
            ]
            []
        ]


viewPlaybackCycler : String -> Msg -> Html Msg
viewPlaybackCycler mode msg =
    label [ class "slider-row" ]
        [ text "Playback "
        , button
            [ class "btn btn-sm"
            , onClick msg
            ]
            [ text mode ]
        ]


viewToggle : String -> Bool -> Msg -> Html Msg
viewToggle name val msg =
    label [ class "slider-row" ]
        [ text (name ++ " ")
        , button
            [ class
                (if val then
                    "btn btn-sm btn-active"

                 else
                    "btn btn-sm"
                )
            , onClick msg
            ]
            [ text
                (if val then
                    "ON"

                 else
                    "OFF"
                )
            ]
        ]



-- Source panel
--
-- Wraps content with a header naming the Source.  Today the only
-- Source is the local instance (always named "localhost"); when
-- remote sources land, each one will get its own panel with a
-- connection-status badge and playback toggle in the header.


viewSourcePanel : String -> Bool -> List (Html Msg) -> Html Msg
viewSourcePanel name headless content =
    div [ class "source-panel" ]
        (div [ class "source-panel-header" ]
            [ span [ class "source-panel-name" ] [ text name ]
            , if headless then
                span
                    [ class "source-panel-badge"
                    , title "This Source has no audio device; its state can be rendered by another instance."
                    ]
                    [ text "no audio" ]

              else
                text ""
            ]
            :: content
        )


{-| Render a Remote Source panel: header with name, connection
status, and a (display-only for now) playback toggle, plus a
read-only list of the remote's heartbeats. Editing controls are
intentionally absent — remote configuration is owned by the remote
instance.
-}
viewRemoteSourcePanel : Set String -> SourceInfo -> Html Msg
viewRemoteSourcePanel pendingRemove source =
    case source.kind of
        Local ->
            text ""

        Remote remote ->
            let
                ( statusClass, statusLabel, statusError ) =
                    case remote.connectionStatus of
                        Connecting ->
                            ( "source-panel-status connecting", "connecting", Nothing )

                        Connected ->
                            ( "source-panel-status connected", "connected", Nothing )

                        Disconnected ->
                            ( "source-panel-status disconnected", "disconnected", Nothing )

                        Error msg ->
                            ( "source-panel-status error", "error", Just msg )

                ( playbackClass, playbackLabel, playbackTitle ) =
                    if remote.playbackEnabled then
                        ( "source-panel-badge playback-on"
                        , "playing"
                        , "Click to stop playing audio for this Source."
                        )

                    else
                        ( "source-panel-badge playback-off"
                        , "muted"
                        , "Click to start playing audio for this Source."
                        )

                removeControl =
                    if Set.member source.name pendingRemove then
                        span [ class "source-remove-confirm" ]
                            [ text "Remove?"
                            , button
                                [ class "btn btn-danger btn-sm"
                                , onClick (ConfirmRemoveSource source.name)
                                ]
                                [ text "Yes" ]
                            , button
                                [ class "btn btn-sm"
                                , onClick (CancelRemoveSource source.name)
                                ]
                                [ text "No" ]
                            ]

                    else
                        button
                            [ class "source-remove-btn"
                            , title "Remove this Remote Source"
                            , onClick (RequestRemoveSource source.name)
                            ]
                            [ text "×" ]
            in
            let
                errorRow =
                    case statusError of
                        Just msg ->
                            [ div [ class "source-panel-error", title msg ] [ text msg ] ]

                        Nothing ->
                            []
            in
            div [ class "source-panel" ] <|
                [ div [ class "source-panel-header" ]
                    [ span [ class "source-panel-name" ] [ text source.name ]
                    , span [ class statusClass, title remote.url ] [ text statusLabel ]
                    , button
                        [ class playbackClass
                        , title playbackTitle
                        , onClick (ToggleRemotePlayback source.name)
                        ]
                        [ text playbackLabel ]
                    , removeControl
                    ]
                ]
                    ++ errorRow
                    ++ [ div [ class "source-panel-body" ]
                            (List.map viewRemoteHeartbeatRow source.heartbeats)
                       ]


{-| The "+ Add source" affordance. When `addSourceForm` is
`Nothing`, renders a small button that opens the form. When the
form is open, replaces the button with an inline two-input form
plus submit/cancel — same inline-disclosure pattern the rest of
the UI uses (no modals).
-}
viewAddSourceButton : Maybe AddSourceForm -> Html Msg
viewAddSourceButton form =
    case form of
        Nothing ->
            button
                [ class "btn btn-add", onClick OpenAddSourceForm ]
                [ text "+ Add Source" ]

        Just f ->
            div [ class "add-source-form" ]
                [ div [ class "add-source-row" ]
                    [ Html.label [ class "add-source-label" ] [ text "URL" ]
                    , input
                        [ class "add-source-input"
                        , Html.Attributes.value f.url
                        , Html.Attributes.placeholder "wss://host/ws"
                        , onInput SetAddSourceUrl
                        ]
                        []
                    ]
                , div [ class "add-source-row" ]
                    [ Html.label [ class "add-source-label" ] [ text "Name" ]
                    , input
                        [ class "add-source-input"
                        , Html.Attributes.value f.name
                        , Html.Attributes.placeholder "(defaults to hostname)"
                        , onInput SetAddSourceName
                        ]
                        []
                    ]
                , case f.error of
                    Just msg ->
                        div [ class "add-source-error" ] [ text msg ]

                    Nothing ->
                        text ""
                , div [ class "add-source-actions" ]
                    [ button [ class "btn", onClick SubmitAddSource ]
                        [ text "Add" ]
                    , button [ class "btn", onClick CloseAddSourceForm ]
                        [ text "Cancel" ]
                    ]
                ]


{-| Flip `playbackEnabled` on the Remote Source named `name`.
No-op on Local sources and on names that don't match anything.
-}
toggleSourcePlayback : String -> SourceInfo -> SourceInfo
toggleSourcePlayback name source =
    if source.name /= name then
        source

    else
        case source.kind of
            Local ->
                source

            Remote remote ->
                { source
                    | kind =
                        Remote
                            { remote
                                | playbackEnabled = not remote.playbackEnabled
                            }
                }


{-| Extract the hostname from a `ws://` or `wss://` URL. Drops
the scheme, the path, the query, and any port — leaving just the
host. Returns the input unchanged when no scheme separator is
present so partial input doesn't fight the user's typing.
-}
parseHostname : String -> String
parseHostname url =
    let
        afterScheme =
            case String.split "://" url of
                _ :: rest :: _ ->
                    rest

                _ ->
                    url

        beforeDelimiter delim s =
            case String.split delim s of
                head :: _ ->
                    head

                _ ->
                    s
    in
    afterScheme
        |> beforeDelimiter "/"
        |> beforeDelimiter "?"
        |> beforeDelimiter "#"
        |> beforeDelimiter ":"


{-| Read the (post-toggle) `playbackEnabled` value for the named
Remote Source so the wire message reflects the new state. Defaults
to False if the name doesn't match — the message would be a no-op
on the backend in that case anyway.
-}
findSourcePlayback : String -> List SourceInfo -> Bool
findSourcePlayback name sources =
    sources
        |> List.filterMap
            (\s ->
                if s.name == name then
                    case s.kind of
                        Remote remote ->
                            Just remote.playbackEnabled

                        Local ->
                            Nothing

                else
                    Nothing
            )
        |> List.head
        |> Maybe.withDefault False


{-| One read-only row inside a Remote Source's panel. Shows the
heartbeat name and its current metric value; no editing affordance.
-}
viewRemoteHeartbeatRow : HeartbeatInfo -> Html Msg
viewRemoteHeartbeatRow hb =
    div [ class "remote-heartbeat-row" ]
        [ span [ class "remote-heartbeat-name" ] [ text hb.name ]
        , span [ class "remote-heartbeat-metric" ]
            [ text (String.fromFloat (toFloat (round (hb.metric * 1000)) / 1000)) ]
        ]



-- Heartbeats


viewHeartbeats : Model -> Html Msg
viewHeartbeats model =
    div [ class "section" ]
        (h2 [] [ text "Heartbeats" ]
            :: List.indexedMap (viewHeartbeatCard model) model.heartbeats
            ++ [ button [ class "btn btn-add", onClick CreateHeartbeat ]
                    [ text "+ Add Heartbeat" ]
               ]
        )


viewHeartbeatCard : Model -> Int -> HeartbeatInfo -> Html Msg
viewHeartbeatCard model index hb =
    let
        collapsed =
            Set.member index model.collapsedHeartbeats

        chevron =
            if collapsed then
                "▸"

            else
                "▾"
    in
    div [ class "card" ]
        [ div [ class "card-header" ]
            [ button
                [ class "card-collapse-btn"
                , onClick (ToggleHeartbeatCollapse index)
                ]
                [ text chevron ]
            , input
                [ class "card-name-input"
                , type_ "text"
                , value hb.name
                , onInput (SetHeartbeatName index)
                ]
                []
            , metricBadge hb.metric hb.tiers
            , if hb.playback /= "clock" then
                span [ class "badge" ] [ text hb.playback ]

              else
                text ""
            , if hb.overridden then
                span [ class "badge badge-warn" ] [ text "override" ]

              else
                text ""
            , viewSparkline (Dict.get index model.metricHistory |> Maybe.withDefault [])
            ]
        , if collapsed then
            text ""

          else
            div [ class "card-body" ]
                [ div [ class "hb-field-row" ]
                    [ label [ class "hb-field-label" ] [ text "Command" ]
                    , input
                        [ class "hb-field-input"
                        , type_ "text"
                        , value hb.command
                        , onInput (SetHeartbeatCommand index)
                        ]
                        []
                    ]
                , div [ class "hb-field-row" ]
                    [ label [ class "hb-field-label" ] [ text "Result mode" ]
                    , select
                        [ class "hb-field-select"
                        , onInput (SetHeartbeatResultMode index)
                        ]
                        [ option [ value "stdout", selected (hb.resultMode == "stdout") ] [ text "stdout" ]
                        , option [ value "exit-code", selected (hb.resultMode == "exit-code") ] [ text "exit-code" ]
                        ]
                    ]
                , viewPlaybackCycler hb.playback (CyclePlayback index)
                , button [ class "btn btn-sm", onClick (TriggerHeartbeat index) ]
                    [ text "Trigger" ]
                , viewSlider "Poll interval" (Just "Seconds between probe command executions.") 1.0 300.0 1.0 hb.pollIntervalSecs (SetHeartbeatSlider PollInterval index)
                , viewSlider "Cycle" (Just "Seconds between plays for one-shot heartbeats.") 1.0 120.0 0.5 hb.cycleSecs (SetHeartbeatSlider CycleSecs index)
                , viewSlider "Offset" (Just "Shifts the heartbeat cycle start time in seconds.") model.sliderRanges.cycleOffset.min model.sliderRanges.cycleOffset.max model.sliderRanges.cycleOffset.step hb.cycleOffsetSecs (SetHeartbeatSlider CycleOffset index)
                , if hb.playback == "continuous" || hb.playback == "loop" then
                    viewSlider "Crossfade ms" (Just "Duration of the crossfade between successive plays, in milliseconds.") model.sliderRanges.crossfadeMs.min model.sliderRanges.crossfadeMs.max model.sliderRanges.crossfadeMs.step hb.crossfadeMs (SetHeartbeatSlider CrossfadeMs index)

                  else
                    text ""
                , if hb.playback == "continuous" then
                    div []
                        [ viewSlider "Phrase gap" (Just "Seconds of silence between phrase repetitions.") 0.0 10.0 0.1 hb.phraseGap (SetHeartbeatSlider PhraseGap index)
                        , viewSlider "Repeat rate" (Just "Speed multiplier on phrase repetition.") 0.01 5.0 0.01 hb.repeatRate (SetHeartbeatSlider RepeatRate index)
                        ]

                  else
                    text ""
                , viewSlider "Value" (Just "Current metric severity. Override to freeze at a fixed value.") model.sliderRanges.overrideMetric.min model.sliderRanges.overrideMetric.max model.sliderRanges.overrideMetric.step hb.metric (OverrideHeartbeat index)
                , div [ class "value-status-row" ]
                    [ span
                        [ class
                            (if hb.overridden then
                                "value-status value-status-overridden"

                             else
                                "value-status"
                            )
                        ]
                        [ text
                            (if hb.overridden then
                                "(overridden)"

                             else
                                "(live)"
                            )
                        ]
                    , if hb.overridden then
                        button
                            [ class "btn btn-sm"
                            , onClick (ClearOverride index)
                            ]
                            [ text "Track Live" ]

                      else
                        text ""
                    ]
                , div [ class "notes-section" ]
                    (List.indexedMap (viewNoteEditor model index (List.length hb.notes)) hb.notes
                        ++ [ button
                                [ class "btn btn-sm"
                                , onClick (AddNote index)
                                ]
                                [ text "Add note" ]
                           ]
                    )
                , viewTierEditor index hb.tiers
                ]
        ]


viewNoteEditor : Model -> Int -> Int -> Int -> NoteInfo -> Html Msg
viewNoteEditor model hbIdx noteCount noteIdx note =
    let
        patchNames =
            Dict.keys model.library |> List.sort
    in
    div [ class "note-panel" ]
        [ div [ class "note-header" ]
            [ span [] [ text ("Note " ++ String.fromInt (noteIdx + 1)) ]
            , if noteCount > 1 then
                button
                    [ class "btn btn-sm transition-remove"
                    , onClick (RemoveNote hbIdx noteIdx)
                    ]
                    [ text "×" ]

              else
                text ""
            ]
        , viewSlider "Volume" (Just "Relative volume of this note within the heartbeat.") model.sliderRanges.noteVolume.min model.sliderRanges.noteVolume.max model.sliderRanges.noteVolume.step note.volume (SetNoteSlider NoteVolume hbIdx noteIdx)
        , viewSlider "Offset" (Just "Delay before this note plays, in seconds.") model.sliderRanges.noteOffset.min model.sliderRanges.noteOffset.max model.sliderRanges.noteOffset.step note.offset (SetNoteSlider NoteOffset hbIdx noteIdx)
        , viewNoteTransitionEdit model.sliderRanges patchNames hbIdx noteIdx note.transition
        ]


viewTierEditor : Int -> List TierInfo -> Html Msg
viewTierEditor hbIdx tiers =
    div [ class "tier-section" ]
        [ div [ class "tier-header" ]
            [ button
                [ class "help-label tier-title"
                , onClick (HelpClicked (Help.Registered "metric-tiers"))
                ]
                [ text "Metric Tiers" ]
            , button
                [ class "btn btn-sm"
                , onClick (AddTier hbIdx)
                ]
                [ text "Add tier" ]
            ]
        , div [] (List.indexedMap (viewTierRow hbIdx (List.length tiers)) tiers)
        ]


viewTierRow : Int -> Int -> Int -> TierInfo -> Html Msg
viewTierRow hbIdx tierCount tierIdx tier =
    div [ class "tier-row" ]
        [ input
            [ type_ "color"
            , value tier.color
            , onInput (SetTierColor hbIdx tierIdx)
            , class "tier-color-input"
            ]
            []
        , input
            [ type_ "text"
            , value tier.label
            , onInput (SetTierLabel hbIdx tierIdx)
            , placeholder "Label"
            , class "tier-label-input"
            ]
            []
        , label [ class "tier-threshold-label" ] [ text "< " ]
        , input
            [ type_ "number"
            , value (String.fromFloat tier.threshold)
            , onInput (SetTierThreshold hbIdx tierIdx)
            , Html.Attributes.step "0.01"
            , Html.Attributes.min "0"
            , Html.Attributes.max "2"
            , class "tier-threshold-input"
            ]
            []
        , if tierCount > 1 then
            button
                [ class "btn btn-sm transition-remove"
                , onClick (RemoveTier hbIdx tierIdx)
                ]
                [ text "×" ]

          else
            text ""
        ]


viewNoteTransitionEdit : SliderRanges -> List String -> Int -> Int -> TransitionInfo -> Html Msg
viewNoteTransitionEdit ranges patchNames hbIdx noteIdx trans =
    let
        currentType =
            case trans of
                Discrete _ ->
                    "discrete"

                Gradient _ ->
                    "gradient"

        typeSwitcher =
            div [ class "transition-header" ]
                [ select
                    [ class "transition-select"
                    , onInput (SwitchTransitionType hbIdx noteIdx)
                    ]
                    [ option [ value "discrete", selected (currentType == "discrete") ] [ text "Discrete" ]
                    , option [ value "gradient", selected (currentType == "gradient") ] [ text "Gradient" ]
                    ]
                ]
    in
    case trans of
        Discrete states ->
            div [ class "transition-edit" ]
                [ typeSwitcher
                , div []
                    (List.indexedMap
                        (viewNoteDiscreteRow ranges patchNames hbIdx noteIdx)
                        states
                    )
                , button
                    [ class "btn btn-sm"
                    , onClick (AddNoteTransitionState hbIdx noteIdx)
                    ]
                    [ text "+" ]
                ]

        Gradient info ->
            let
                segments =
                    syncSegments info.segments (List.length info.patches)

                interleaved =
                    interleave patchNames hbIdx noteIdx ranges info.patches segments
            in
            div [ class "transition-edit" ]
                (typeSwitcher
                    :: interleaved
                    ++ [ button
                            [ class "btn btn-sm"
                            , onClick (AddNoteGradientPatch hbIdx noteIdx)
                            ]
                            [ text "+" ]
                       ]
                )


viewNoteDiscreteRow : SliderRanges -> List String -> Int -> Int -> Int -> { threshold : Float, patch : String } -> Html Msg
viewNoteDiscreteRow ranges patchNames hbIdx noteIdx stateIdx state =
    div [ class "transition-row" ]
        [ select
            [ class "transition-select"
            , onInput (SetNoteTransitionPatch hbIdx noteIdx stateIdx)
            ]
            (List.map
                (\name ->
                    option
                        [ value name
                        , selected (name == state.patch)
                        ]
                        [ text name ]
                )
                patchNames
            )
        , text " < "
        , input
            [ type_ "number"
            , class "transition-input"
            , Html.Attributes.min (String.fromFloat ranges.discreteThreshold.min)
            , Html.Attributes.max (String.fromFloat ranges.discreteThreshold.max)
            , step (String.fromFloat ranges.discreteThreshold.step)
            , value (String.fromFloat state.threshold)
            , onInput (SetNoteTransitionThreshold hbIdx noteIdx stateIdx)
            ]
            []
        , button
            [ class "btn btn-sm transition-remove"
            , onClick (RemoveNoteTransitionState hbIdx noteIdx stateIdx)
            ]
            [ text "×" ]
        ]


viewNoteGradientRow : List String -> Int -> Int -> Int -> String -> Html Msg
viewNoteGradientRow patchNames hbIdx noteIdx patchIdx patchName =
    div [ class "transition-row" ]
        [ select
            [ class "transition-select"
            , onInput (SetNoteTransitionPatch hbIdx noteIdx patchIdx)
            ]
            (List.map
                (\name ->
                    option
                        [ value name
                        , selected (name == patchName)
                        ]
                        [ text name ]
                )
                patchNames
            )
        , button
            [ class "btn btn-sm transition-remove"
            , onClick (RemoveNoteGradientPatch hbIdx noteIdx patchIdx)
            ]
            [ text "×" ]
        ]


interleave : List String -> Int -> Int -> SliderRanges -> List String -> List LerpStrategy -> List (Html Msg)
interleave patchNames hbIdx noteIdx ranges patches segments =
    let
        patchRows =
            List.indexedMap (viewNoteGradientRow patchNames hbIdx noteIdx) patches

        segRows =
            List.indexedMap (viewSegmentEditor ranges hbIdx noteIdx) segments
    in
    List.concatMap identity
        (List.indexedMap
            (\i pRow ->
                if i < List.length segRows then
                    case getAt i segRows of
                        Just sRow ->
                            [ pRow, sRow ]

                        Nothing ->
                            [ pRow ]

                else
                    [ pRow ]
            )
            patchRows
        )


viewSegmentEditor : SliderRanges -> Int -> Int -> Int -> LerpStrategy -> Html Msg
viewSegmentEditor ranges hbIdx noteIdx segIdx strat =
    let
        currentName =
            strategyName strat

        currentIntensity =
            strategyIntensity strat

        isStep =
            case strat of
                Step _ ->
                    True

                _ ->
                    False

        intensityRange =
            if isStep then
                ranges.stepPosition

            else
                ranges.segmentIntensity
    in
    div [ class "segment-editor" ]
        [ div [ class "segment-controls" ]
            [ select
                [ class "transition-select"
                , onInput (SetSegmentStrategy hbIdx noteIdx segIdx)
                ]
                [ option [ value "linear", selected (currentName == "linear") ] [ text "Linear" ]
                , option [ value "ease-in", selected (currentName == "ease-in") ] [ text "Ease In" ]
                , option [ value "ease-out", selected (currentName == "ease-out") ] [ text "Ease Out" ]
                , option [ value "ease-in-out", selected (currentName == "ease-in-out") ] [ text "Ease In/Out" ]
                , option [ value "step", selected (currentName == "step") ] [ text "Step" ]
                ]
            , input
                [ type_ "range"
                , class "segment-intensity-slider"
                , Html.Attributes.min (String.fromFloat intensityRange.min)
                , Html.Attributes.max (String.fromFloat intensityRange.max)
                , step (String.fromFloat intensityRange.step)
                , value (String.fromFloat currentIntensity)
                , onInput (SetSegmentIntensity hbIdx noteIdx segIdx)
                ]
                []
            , input
                [ type_ "number"
                , class "transition-input"
                , Html.Attributes.min (String.fromFloat intensityRange.min)
                , Html.Attributes.max (String.fromFloat intensityRange.max)
                , step (String.fromFloat intensityRange.step)
                , value (String.fromFloat currentIntensity)
                , onInput (SetSegmentIntensity hbIdx noteIdx segIdx)
                ]
                []
            ]
        , viewStrategySvg strat
        ]


viewStrategySvg : LerpStrategy -> Html msg
viewStrategySvg strat =
    let
        padding =
            2.0

        w =
            60.0

        h =
            40.0

        innerW =
            w - 2 * padding

        innerH =
            h - 2 * padding

        samples =
            20

        points =
            List.map
                (\i ->
                    let
                        t =
                            toFloat i / toFloat samples

                        y =
                            applyStrategy strat t

                        px =
                            padding + t * innerW

                        py =
                            padding + (1 - y) * innerH
                    in
                    String.fromFloat px ++ "," ++ String.fromFloat py
                )
                (List.range 0 samples)

        polylinePoints =
            String.join " " points
    in
    Svg.svg
        [ SA.viewBox ("0 0 " ++ String.fromFloat w ++ " " ++ String.fromFloat h)
        , SA.width "60"
        , SA.height "40"
        , SA.class "segment-svg"
        ]
        [ Svg.line
            [ SA.x1 (String.fromFloat padding)
            , SA.y1 (String.fromFloat padding)
            , SA.x2 (String.fromFloat padding)
            , SA.y2 (String.fromFloat (h - padding))
            , SA.stroke "var(--color-border)"
            , SA.strokeWidth "1"
            ]
            []
        , Svg.line
            [ SA.x1 (String.fromFloat padding)
            , SA.y1 (String.fromFloat (h - padding))
            , SA.x2 (String.fromFloat (w - padding))
            , SA.y2 (String.fromFloat (h - padding))
            , SA.stroke "var(--color-border)"
            , SA.strokeWidth "1"
            ]
            []
        , Svg.polyline
            [ SA.points polylinePoints
            , SA.fill "none"
            , SA.stroke "var(--color-accent)"
            , SA.strokeWidth "1.5"
            ]
            []
        ]


viewSparkline : List Float -> Html msg
viewSparkline values =
    let
        h =
            20.0

        children =
            if List.length values < 2 then
                []

            else
                let
                    linePoints =
                        values
                            |> List.indexedMap
                                (\i v ->
                                    String.fromFloat (toFloat i * 3.0)
                                        ++ ","
                                        ++ String.fromFloat (h - v * h)
                                )
                            |> String.join " "

                    fillPoints =
                        "0,"
                            ++ String.fromFloat h
                            ++ " "
                            ++ linePoints
                            ++ " "
                            ++ String.fromFloat ((toFloat (List.length values) - 1) * 3.0)
                            ++ ","
                            ++ String.fromFloat h
                in
                [ Svg.polygon
                    [ SA.points fillPoints
                    , SA.fill "var(--color-accent)"
                    , SA.fillOpacity "0.1"
                    , SA.stroke "none"
                    ]
                    []
                , Svg.polyline
                    [ SA.points linePoints
                    , SA.fill "none"
                    , SA.stroke "var(--color-accent)"
                    , SA.strokeWidth "1.5"
                    ]
                    []
                ]

        w =
            toFloat (List.length values) * 3.0 |> Basics.max 1.0
    in
    Svg.svg
        [ SA.viewBox ("0 0 " ++ String.fromFloat w ++ " " ++ String.fromFloat h)
        , Html.Attributes.attribute "preserveAspectRatio" "none"
        , Html.Attributes.style "flex" "1"
        , Html.Attributes.style "margin-left" "auto"
        , Html.Attributes.style "height" "24px"
        , Html.Attributes.style "border" "1px solid var(--color-border)"
        , Html.Attributes.style "border-radius" "4px"
        ]
        children


resolveTier : Float -> List TierInfo -> Maybe TierInfo
resolveTier m tiers =
    case tiers of
        [] ->
            Nothing

        t :: rest ->
            if m < t.threshold then
                Just t

            else
                case rest of
                    [] ->
                        Just t

                    _ ->
                        resolveTier m rest


metricBadge : Float -> List TierInfo -> Html msg
metricBadge m tiers =
    case resolveTier m tiers of
        Just tier ->
            span
                [ class "metric"
                , style "background" (tier.color ++ "33")
                , style "color" tier.color
                ]
                [ text tier.label ]

        Nothing ->
            span [ class "metric" ]
                [ text (formatMetric m) ]


formatMetric : Float -> String
formatMetric m =
    let
        rounded =
            toFloat (round (m * 1000)) / 1000
    in
    String.fromFloat rounded



-- Probe log


viewProbeLog : Model -> Html Msg
viewProbeLog model =
    div [ class "section" ]
        [ h2 [] [ text "Probe Log" ]
        , div [ class "log-container" ]
            (List.map (viewLogEntry model.timezone model.heartbeats) model.probeLog)
        ]


viewLogEntry : Time.Zone -> List HeartbeatInfo -> ProbeLogEntry -> Html Msg
viewLogEntry zone heartbeats entry =
    let
        tierColor =
            heartbeats
                |> List.filter (\hb -> hb.name == entry.name)
                |> List.head
                |> Maybe.andThen
                    (\hb ->
                        hb.tiers
                            |> List.filter (\t -> t.label == entry.result)
                            |> List.head
                    )
                |> Maybe.map .color

        resultAttrs =
            case tierColor of
                Just c ->
                    [ class "log-result", style "color" c ]

                Nothing ->
                    [ class (logResultClass entry.result) ]
    in
    div [ class "log-entry" ]
        [ span [ class "log-timestamp" ] [ text (formatTimestamp zone entry.timestamp) ]
        , span [ class "log-name" ] [ text entry.name ]
        , span resultAttrs [ text entry.result ]
        , if entry.overridden then
            span [ class "badge badge-warn" ] [ text "override" ]

          else
            text ""
        ]


formatTimestamp : Time.Zone -> Float -> String
formatTimestamp zone epochSecs =
    let
        posix =
            Time.millisToPosix (round (epochSecs * 1000))

        h =
            String.padLeft 2 '0' (String.fromInt (Time.toHour zone posix))

        m =
            String.padLeft 2 '0' (String.fromInt (Time.toMinute zone posix))

        s =
            String.padLeft 2 '0' (String.fromInt (Time.toSecond zone posix))
    in
    h ++ ":" ++ m ++ ":" ++ s


logResultClass : String -> String
logResultClass result =
    case result of
        "healthy" ->
            "log-result log-healthy"

        "degraded" ->
            "log-result log-degraded"

        "down" ->
            "log-result log-down"

        _ ->
            "log-result"



-- Patch list and editor


viewPatchList : Model -> Html Msg
viewPatchList model =
    let
        activePatchNames =
            List.concatMap transitionPatchNames model.heartbeats
                |> unique

        allNames =
            Dict.keys model.library |> List.sort

        ( active, rest ) =
            List.partition (\n -> List.member n activePatchNames) allNames
    in
    div [ class "section" ]
        [ div [ class "section-header" ]
            [ h2 [] [ text "Patch Library" ]
            , button
                [ class "patch-action-btn"
                , onClick CreatePatch
                , title "New patch"
                ]
                [ text "+" ]
            ]
        , div [ class "patch-list" ]
            (List.map (viewPatchItem model) active
                ++ List.map (viewPatchItem model) rest
            )
        ]


viewPatchItem : Model -> String -> Html Msg
viewPatchItem model name =
    let
        isSelected =
            model.selectedPatch == Just name

        isRenaming =
            model.renamingPatch == Just name

        overrideLabel =
            case Dict.get name model.overrides of
                Just info ->
                    span [ class "patch-item-base" ]
                        [ text (" ← " ++ info.base) ]

                Nothing ->
                    text ""

        isOverride =
            Dict.member name model.overrides
    in
    div
        [ class
            (if isSelected then
                "patch-item selected"

             else
                "patch-item"
            )
        , onClick (SelectPatch name)
        ]
        [ div [ class "patch-item-row" ]
            [ if isRenaming then
                input
                    [ type_ "text"
                    , class "rename-input"
                    , value model.renameInput
                    , onInput SetRenameInput
                    , Html.Events.stopPropagationOn "click"
                        (Decode.succeed ( NoOp, True ))
                    , onEnter (ConfirmRename name)
                    , onEsc CancelRename
                    ]
                    []

              else
                span [ class "patch-item-name" ]
                    [ text name, overrideLabel ]
            , div [ class "patch-item-actions" ]
                (if isRenaming then
                    [ button
                        [ class "patch-action-btn"
                        , onClick (ConfirmRename name)
                        , title "Confirm rename"
                        ]
                        [ text "✓" ]
                    , button
                        [ class "patch-action-btn"
                        , onClick CancelRename
                        , title "Cancel rename"
                        ]
                        [ text "✗" ]
                    ]

                 else
                    [ button
                        [ class "patch-action-btn"
                        , Html.Events.stopPropagationOn "click"
                            (Decode.succeed ( StartRename name, True ))
                        , title "Rename"
                        ]
                        [ text "✎" ]
                    , if not isOverride then
                        button
                            [ class "patch-action-btn"
                            , Html.Events.stopPropagationOn "click"
                                (Decode.succeed ( CreateOverride name, True ))
                            , title "Create override"
                            ]
                            [ text "+▶" ]

                      else
                        text ""
                    ]
                )
            ]
        ]


transitionPatchNames : HeartbeatInfo -> List String
transitionPatchNames hb =
    List.concatMap noteTransitionPatchNames hb.notes


noteTransitionPatchNames : NoteInfo -> List String
noteTransitionPatchNames note =
    case note.transition of
        Discrete states ->
            List.map .patch states

        Gradient info ->
            info.patches


viewPatchEditor : Model -> Html Msg
viewPatchEditor model =
    case model.selectedPatch of
        Nothing ->
            div [ class "section" ]
                [ text "Select a patch to edit." ]

        Just patchName ->
            case Dict.get patchName model.library of
                Nothing ->
                    div [ class "section" ]
                        [ text ("Patch not found: " ++ patchName) ]

                Just patchValues ->
                    let
                        maybeOverride =
                            Dict.get patchName model.overrides

                        headerExtra =
                            case maybeOverride of
                                Just info ->
                                    div [ class "override-header" ]
                                        [ text ("overrides: " ++ info.base) ]

                                Nothing ->
                                    text ""
                    in
                    div [ class "section patch-editor" ]
                        [ h2 [] [ text patchName ]
                        , headerExtra
                        , label [ class "play-on-change" ]
                            [ input
                                [ type_ "checkbox"
                                , Html.Attributes.checked (Set.member patchName model.playOnChange)
                                , onClick (TogglePlayOnChange patchName)
                                ]
                                []
                            , text " Play on change"
                            ]
                        , div [ class "param-grid" ]
                            (List.map
                                (viewParamSlider model patchName patchValues maybeOverride)
                                model.patchParamMeta
                            )
                        ]


viewParamSlider :
    Model
    -> String
    -> Dict String Float
    -> Maybe OverrideInfo
    -> PatchParamMeta
    -> Html Msg
viewParamSlider model patchName patchValues maybeOverride meta =
    let
        val =
            Dict.get meta.name patchValues
                |> Maybe.withDefault 0.0

        ( inherited, resetBtn ) =
            case maybeOverride of
                Just info ->
                    if Dict.member meta.name info.delta then
                        ( False
                        , button
                            [ class "param-reset-btn"
                            , onClick (ResetOverrideParam patchName meta.name)
                            , title "Reset to inherited value"
                            ]
                            [ text "×" ]
                        )

                    else
                        ( True, text "" )

                Nothing ->
                    ( False, text "" )

        -- Param labels are `help-label` buttons when the backend
        -- ships a description; empty descriptions leave a plain
        -- label (no click affordance, because there's nothing to
        -- show).  The `help-label` class strips button chrome so
        -- the label still looks like a param label next to its
        -- slider.  See `Help.Source.Literal` — descriptions come
        -- from `PatchParamMeta` over the wire, not from the
        -- frontend-local `Help.lookup` table.
        labelNode =
            if String.isEmpty meta.description then
                label [ class "param-label" ] [ text meta.name ]

            else
                button
                    [ class "param-label help-label"
                    , onClick (HelpClicked (Help.Literal meta.description))
                    ]
                    [ text meta.name ]
    in
    div
        [ class
            (if inherited then
                "param-slider param-inherited"

             else
                "param-slider"
            )
        ]
        [ labelNode
        , input
            [ type_ "range"
            , Html.Attributes.min (String.fromFloat meta.min)
            , Html.Attributes.max (String.fromFloat meta.max)
            , step (String.fromFloat meta.step)
            , value (String.fromFloat val)
            , onInput (SetPatchParam patchName meta.name)
            ]
            []
        , input
            [ type_ "number"
            , class "num-input"
            , Html.Attributes.min (String.fromFloat meta.min)
            , Html.Attributes.max (String.fromFloat meta.max)
            , step (String.fromFloat meta.step)
            , value (String.fromFloat val)
            , onInput (SetPatchParam patchName meta.name)
            ]
            []
        , resetBtn
        ]



-- Import


viewImport : Model -> Html Msg
viewImport model =
    div [ class "section" ]
        [ h2 [] [ text "Import" ]
        , textarea
            [ class "import-textarea"
            , placeholder "Paste TOML or JSON patches here..."
            , value model.importText
            , onInput SetImportText
            ]
            []
        , button [ class "btn", onClick SubmitImport ]
            [ text "Import" ]
        , case model.importError of
            Just err ->
                div [ class "error" ] [ text err ]

            Nothing ->
                text ""
        ]



-- Export inline


viewExportInline : Model -> Html Msg
viewExportInline model =
    case model.exportData of
        Nothing ->
            text ""

        Just exportText ->
            div [ class "export-inline" ]
                [ div [ class "export-inline-header" ]
                    [ span []
                        [ text
                            ("Exported Configuration ("
                                ++ String.toUpper model.exportFormat
                                ++ ")"
                            )
                        ]
                    , div [ class "export-inline-actions" ]
                        [ button [ class "btn btn-sm", onClick CopyExport ]
                            [ text
                                (if model.copyFeedback then
                                    "Copied!"

                                 else
                                    "Copy"
                                )
                            ]
                        , button [ class "btn btn-sm", onClick DismissExport ]
                            [ text "Close" ]
                        ]
                    ]
                , pre [ class "export-pre" ]
                    [ text exportText ]
                ]



-- Protocol error


viewProtocolError : Model -> Html Msg
viewProtocolError model =
    case model.protocolError of
        Nothing ->
            text ""

        Just err ->
            div [ class "error-banner", onClick DismissProtocolError ]
                [ text ("Protocol error: " ++ err) ]



-- Me page


viewMe : Model -> Html Msg
viewMe model =
    div [ class "container" ]
        [ h1 [] [ text "User Info" ]
        , case model.me of
            MeLoading ->
                text "Loading..."

            MeLoaded info ->
                div []
                    [ p [] [ text ("Name: " ++ info.name) ]
                    , p []
                        [ text
                            ("Auth: "
                                ++ (if info.authEnabled then
                                        "OIDC"

                                    else
                                        "disabled"
                                   )
                            )
                        ]
                    ]

            MeFailed ->
                text "Failed to load user info."
        ]
