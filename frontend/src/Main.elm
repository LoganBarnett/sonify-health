module Main exposing (main)

import Browser
import Browser.Navigation as Nav
import Dict exposing (Dict)
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (onClick, onInput)
import Http
import Json.Decode as Decode
import Ports
import Process
import Protocol exposing (..)
import Svg
import Svg.Attributes as SA
import Task
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
    , heartbeatLoop : Bool
    , probeLog : List ProbeLogEntry
    , exportData : Maybe (Dict String (Dict String Float))
    , debounces : Dict String Int
    , nextDebounce : Int
    , importText : String
    , importError : Maybe String
    , protocolError : Maybe String
    , sliderRanges : SliderRanges
    , renamingPatch : Maybe String
    , renameInput : String
    , configWritable : Bool
    , configPath : Maybe String
    , saveStatus : Maybe String
    }


type Msg
    = UrlRequested Browser.UrlRequest
    | UrlChanged Url
    | GotMe (Result Http.Error MeInfo)
    | WebSocketReceived String
    | SetPatchParam String String String
    | PatchParamDebounce String String Int Float
    | ToggleMute
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
    | ClearOverride Int
    | ToggleContinuous Int
    | ToggleHeartbeatLoop
    | TriggerHeartbeat
    | RevertAll
    | SelectPatch String
    | Export
    | DismissExport
    | SetImportText String
    | SubmitImport
    | SetHeartbeatSlider HeartbeatSlider Int String
    | HeartbeatSliderDebounce HeartbeatSlider Int Int Float
    | CreateOverride String
    | StartRename String
    | SetRenameInput String
    | ConfirmRename String
    | CancelRename
    | ResetOverrideParam String String
    | SaveConfig
    | DismissSaveStatus
    | DismissProtocolError
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
      , heartbeatLoop = False
      , probeLog = []
      , exportData = Nothing
      , debounces = Dict.empty
      , nextDebounce = 0
      , importText = ""
      , importError = Nothing
      , protocolError = Nothing
      , sliderRanges = defaultSliderRanges
      , renamingPatch = Nothing
      , renameInput = ""
      , configWritable = False
      , configPath = Nothing
      , saveStatus = Nothing
      }
    , cmdForRoute route
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
                ( model
                , Ports.websocketSend
                    (encodeSetPatchParam patchName param val)
                )

            else
                ( model, Cmd.none )

        ToggleMute ->
            ( model
            , Ports.websocketSend (encodeSetMuted (not model.muted))
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
                    ( model
                    , Ports.websocketSend (encodeOverrideHeartbeat index val)
                    )

                Nothing ->
                    ( model, Cmd.none )

        ClearOverride index ->
            ( model
            , Ports.websocketSend (encodeClearOverride index)
            )

        ToggleContinuous index ->
            let
                hb =
                    List.drop index model.heartbeats
                        |> List.head
            in
            case hb of
                Just h ->
                    ( model
                    , Ports.websocketSend
                        (encodeSetContinuous index (not h.continuous))
                    )

                Nothing ->
                    ( model, Cmd.none )

        ToggleHeartbeatLoop ->
            ( model
            , Ports.websocketSend
                (encodeSetHeartbeatLoop (not model.heartbeatLoop))
            )

        TriggerHeartbeat ->
            ( model, Ports.websocketSend encodeTriggerHeartbeat )

        RevertAll ->
            ( model, Ports.websocketSend encodeRevertAll )

        SelectPatch name ->
            ( { model | selectedPatch = Just name }, Cmd.none )

        Export ->
            ( model, Ports.websocketSend encodeExportConfig )

        DismissExport ->
            ( { model | exportData = Nothing }, Cmd.none )

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
                , heartbeatLoop = state.heartbeatLoop
                , heartbeats = state.heartbeats
                , sliderRanges = state.sliderRanges
                , configWritable = state.configWritable
                , configPath = state.configPath
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
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb -> { hb | metric = value })
                        model.heartbeats
              }
            , Cmd.none
            )

        OverrideChanged index _ overridden ->
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb -> { hb | overridden = overridden })
                        model.heartbeats
              }
            , Cmd.none
            )

        HeartbeatLoopChanged enabled ->
            ( { model | heartbeatLoop = enabled }, Cmd.none )

        ContinuousChanged index value ->
            ( { model
                | heartbeats =
                    updateAt index
                        (\hb -> { hb | continuous = value })
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

        ProbeLog entry ->
            ( { model | probeLog = entry :: List.take 99 model.probeLog }
            , Cmd.none
            )

        ConfigExport lib _ ->
            ( { model | exportData = Just lib }, Cmd.none )

        ImportError err ->
            ( { model | importError = Just err }, Cmd.none )

        ConfigSaved ->
            ( { model | saveStatus = Just "Saved" }
            , Process.sleep 2000
                |> Task.perform (\_ -> DismissSaveStatus)
            )

        SaveError err ->
            ( { model | saveStatus = Just err }, Cmd.none )

        Connected ->
            ( { model | connected = True }
            , Ports.websocketSend encodeGetState
            )

        Disconnected ->
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


setHeartbeatSliderValue : HeartbeatSlider -> Float -> HeartbeatInfo -> HeartbeatInfo
setHeartbeatSliderValue slider val hb =
    case slider of
        CycleOffset ->
            { hb | cycleOffsetSecs = val }

        CrossfadeMs ->
            { hb | crossfadeMs = val }


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
        ]
    }


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
    div [ class "app-layout" ]
        [ viewToolbar model
        , div [ class "split-panel" ]
            [ div [ class "panel-left" ]
                [ viewHeartbeats model
                , viewProbeLog model
                , viewImport model
                ]
            , div [ class "panel-right" ]
                [ viewPatchList model
                , viewPatchEditor model
                ]
            ]
        , viewExportModal model
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
        , viewSlider "Master" model.sliderRanges.masterVolume.min model.sliderRanges.masterVolume.max model.sliderRanges.masterVolume.step model.masterVolume SetMasterVolume
        , button [ class "btn", onClick TriggerHeartbeat ]
            [ text "Trigger" ]
        , button
            [ class
                (if model.heartbeatLoop then
                    "btn btn-active"

                 else
                    "btn"
                )
            , onClick ToggleHeartbeatLoop
            ]
            [ text
                (if model.heartbeatLoop then
                    "Loop: ON"

                 else
                    "Loop: OFF"
                )
            ]
        , button [ class "btn", onClick RevertAll ]
            [ text "Revert" ]
        , button
            (if model.configWritable then
                [ class "btn", onClick SaveConfig ]

             else
                [ class "btn"
                , Html.Attributes.disabled True
                , title
                    (case model.configPath of
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
        , button [ class "btn", onClick Export ]
            [ text "Export" ]
        ]



-- Shared slider + number input component.


viewSlider : String -> Float -> Float -> Float -> Float -> (String -> Msg) -> Html Msg
viewSlider name min_ max_ step_ val toMsg =
    label [ class "slider-row" ]
        [ text (name ++ " ")
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



-- Heartbeats


viewHeartbeats : Model -> Html Msg
viewHeartbeats model =
    div [ class "section" ]
        (h2 [] [ text "Heartbeats" ]
            :: List.indexedMap (viewHeartbeatCard model) model.heartbeats
        )


viewHeartbeatCard : Model -> Int -> HeartbeatInfo -> Html Msg
viewHeartbeatCard model index hb =
    div [ class "card" ]
        [ div [ class "card-header" ]
            [ span [ class "card-name" ] [ text hb.name ]
            , span [ class (metricClass hb.metric) ]
                [ text (metricLabel hb.metric) ]
            , if hb.continuous then
                span [ class "badge" ] [ text "continuous" ]

              else
                text ""
            , if hb.overridden then
                span [ class "badge badge-warn" ] [ text "override" ]

              else
                text ""
            ]
        , div [ class "card-body" ]
            [ viewToggle "Continuous" hb.continuous (ToggleContinuous index)
            , viewSlider "Offset" model.sliderRanges.cycleOffset.min model.sliderRanges.cycleOffset.max model.sliderRanges.cycleOffset.step hb.cycleOffsetSecs (SetHeartbeatSlider CycleOffset index)
            , if hb.continuous then
                viewSlider "Crossfade ms" model.sliderRanges.crossfadeMs.min model.sliderRanges.crossfadeMs.max model.sliderRanges.crossfadeMs.step hb.crossfadeMs (SetHeartbeatSlider CrossfadeMs index)

              else
                text ""
            , viewSlider "Override" model.sliderRanges.overrideMetric.min model.sliderRanges.overrideMetric.max model.sliderRanges.overrideMetric.step hb.metric (OverrideHeartbeat index)
            , if hb.overridden then
                button
                    [ class "btn btn-sm"
                    , onClick (ClearOverride index)
                    ]
                    [ text "Live" ]

              else
                text ""
            , div [ class "notes-section" ]
                (List.indexedMap (viewNoteEditor model index (List.length hb.notes)) hb.notes
                    ++ [ button
                            [ class "btn btn-sm"
                            , onClick (AddNote index)
                            ]
                            [ text "Add note" ]
                       ]
                )
            ]
        ]


viewNoteEditor : Model -> Int -> Int -> Int -> NoteInfo -> Html Msg
viewNoteEditor model hbIdx noteCount noteIdx note =
    let
        patchNames =
            Dict.keys model.library |> List.sort
    in
    div [ class "note-editor" ]
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
        , viewSlider "Volume" model.sliderRanges.noteVolume.min model.sliderRanges.noteVolume.max model.sliderRanges.noteVolume.step note.volume (SetNoteSlider NoteVolume hbIdx noteIdx)
        , viewSlider "Offset" model.sliderRanges.noteOffset.min model.sliderRanges.noteOffset.max model.sliderRanges.noteOffset.step note.offset (SetNoteSlider NoteOffset hbIdx noteIdx)
        , viewNoteTransitionEdit model.sliderRanges patchNames hbIdx noteIdx note.transition
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


metricClass : Float -> String
metricClass m =
    if m < 0.25 then
        "metric metric-healthy"

    else if m < 0.75 then
        "metric metric-degraded"

    else
        "metric metric-down"


metricLabel : Float -> String
metricLabel m =
    if m < 0.25 then
        "healthy"

    else if m < 0.75 then
        "degraded"

    else
        "down"



-- Probe log


viewProbeLog : Model -> Html Msg
viewProbeLog model =
    div [ class "section" ]
        [ h2 [] [ text "Probe Log" ]
        , div [ class "log-container" ]
            (List.map viewLogEntry model.probeLog)
        ]


viewLogEntry : ProbeLogEntry -> Html Msg
viewLogEntry entry =
    div [ class "log-entry" ]
        [ span [ class "log-name" ] [ text entry.name ]
        , span [ class (logResultClass entry.result) ]
            [ text entry.result ]
        , if entry.overridden then
            span [ class "badge badge-warn" ] [ text "override" ]

          else
            text ""
        ]


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
        [ h2 [] [ text "Patch Library" ]
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
                        , div [ class "param-grid" ]
                            (List.map
                                (viewParamSlider patchName patchValues maybeOverride)
                                model.patchParamMeta
                            )
                        ]


viewParamSlider :
    String
    -> Dict String Float
    -> Maybe OverrideInfo
    -> PatchParamMeta
    -> Html Msg
viewParamSlider patchName patchValues maybeOverride meta =
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
    in
    div
        [ class
            (if inherited then
                "param-slider param-inherited"

             else
                "param-slider"
            )
        ]
        [ label [ class "param-label", title meta.description ]
            [ text meta.name ]
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



-- Export modal


viewExportModal : Model -> Html Msg
viewExportModal model =
    case model.exportData of
        Nothing ->
            text ""

        Just lib ->
            div [ class "modal-backdrop", onClick DismissExport ]
                [ div
                    [ class "modal"
                    , Html.Events.stopPropagationOn "click"
                        (Decode.succeed ( NoOp, True ))
                    ]
                    [ h2 [] [ text "Exported Configuration" ]
                    , pre [ class "export-pre" ]
                        [ text (libraryToToml lib) ]
                    , button [ class "btn", onClick DismissExport ]
                        [ text "Close" ]
                    ]
                ]


libraryToToml : Dict String (Dict String Float) -> String
libraryToToml lib =
    Dict.toList lib
        |> List.map
            (\( name, params ) ->
                "[patches."
                    ++ name
                    ++ "]\n"
                    ++ (Dict.toList params
                            |> List.map
                                (\( k, v ) ->
                                    k ++ " = " ++ String.fromFloat v
                                )
                            |> String.join "\n"
                       )
            )
        |> String.join "\n\n"



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
