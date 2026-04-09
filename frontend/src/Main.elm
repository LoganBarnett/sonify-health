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
    | SetHeartbeatVolume Int String
    | HeartbeatVolDebounce Int Int Float
    | OverrideHeartbeat Int String
    | ClearOverride Int
    | ToggleHeartbeatLoop
    | TriggerHeartbeat
    | RevertAll
    | SelectPatch String
    | Export
    | DismissExport
    | SetImportText String
    | SubmitImport
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
      }
    , cmdForRoute route
    )


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
                    in
                    debounce key val model (PatchParamDebounce patchName param)

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
                    debounce "master_vol" val model MasterVolDebounce

                Nothing ->
                    ( model, Cmd.none )

        MasterVolDebounce id val ->
            if isCurrentDebounce "master_vol" id model then
                ( model
                , Ports.websocketSend (encodeSetMasterVolume val)
                )

            else
                ( model, Cmd.none )

        SetHeartbeatVolume index rawVal ->
            case String.toFloat rawVal of
                Just val ->
                    let
                        key =
                            "hb_vol:" ++ String.fromInt index
                    in
                    debounce key val model (HeartbeatVolDebounce index)

                Nothing ->
                    ( model, Cmd.none )

        HeartbeatVolDebounce index id val ->
            let
                key =
                    "hb_vol:" ++ String.fromInt index
            in
            if isCurrentDebounce key id model then
                ( model
                , Ports.websocketSend (encodeSetHeartbeatVolume index val)
                )

            else
                ( model, Cmd.none )

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
                , muted = state.muted
                , masterVolume = state.masterVolume
                , heartbeatLoop = state.heartbeatLoop
                , heartbeats = state.heartbeats
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

        VolumeChanged layer maybeIndex volume ->
            case ( layer, maybeIndex ) of
                ( "master", _ ) ->
                    ( { model | masterVolume = volume }, Cmd.none )

                ( "heartbeat", Just index ) ->
                    ( { model
                        | heartbeats =
                            updateAt index
                                (\hb -> { hb | volume = volume })
                                model.heartbeats
                      }
                    , Cmd.none
                    )

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

        LibraryChanged lib ->
            ( { model | library = lib }, Cmd.none )

        ProbeLog entry ->
            ( { model | probeLog = entry :: List.take 99 model.probeLog }
            , Cmd.none
            )

        ConfigExport lib ->
            ( { model | exportData = Just lib }, Cmd.none )

        ImportError err ->
            ( { model | importError = Just err }, Cmd.none )

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
        , label [ class "toolbar-slider" ]
            [ text "Master "
            , input
                [ type_ "range"
                , Html.Attributes.min "0"
                , Html.Attributes.max "1"
                , step "0.01"
                , value (String.fromFloat model.masterVolume)
                , onInput SetMasterVolume
                ]
                []
            , text (String.left 4 (String.fromFloat model.masterVolume))
            ]
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
        , button [ class "btn", onClick Export ]
            [ text "Export" ]
        ]



-- Heartbeats


viewHeartbeats : Model -> Html Msg
viewHeartbeats model =
    div [ class "section" ]
        (h2 [] [ text "Heartbeats" ]
            :: List.indexedMap (viewHeartbeatCard model) model.heartbeats
        )


viewHeartbeatCard : Model -> Int -> HeartbeatInfo -> Html Msg
viewHeartbeatCard _ index hb =
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
            [ label [ class "slider-row" ]
                [ text "Volume "
                , input
                    [ type_ "range"
                    , Html.Attributes.min "0"
                    , Html.Attributes.max "1"
                    , step "0.01"
                    , value (String.fromFloat hb.volume)
                    , onInput (SetHeartbeatVolume index)
                    ]
                    []
                , text (String.left 4 (String.fromFloat hb.volume))
                ]
            , label [ class "slider-row" ]
                [ text "Override "
                , input
                    [ type_ "range"
                    , Html.Attributes.min "0"
                    , Html.Attributes.max "1"
                    , step "0.01"
                    , value (String.fromFloat hb.metric)
                    , onInput (OverrideHeartbeat index)
                    ]
                    []
                , text (String.left 5 (String.fromFloat hb.metric))
                , if hb.overridden then
                    button
                        [ class "btn btn-sm"
                        , onClick (ClearOverride index)
                        ]
                        [ text "Live" ]

                  else
                    text ""
                ]
            , viewTransition hb.transition
            ]
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


viewTransition : TransitionInfo -> Html Msg
viewTransition trans =
    case trans of
        Discrete states ->
            div [ class "transition-info" ]
                [ text "Discrete: "
                , span []
                    (List.map
                        (\s ->
                            span [ class "transition-state" ]
                                [ a
                                    [ href "#"
                                    , onClick (SelectPatch s.patch)
                                    ]
                                    [ text s.patch ]
                                , text
                                    (" (<"
                                        ++ String.left 4 (String.fromFloat s.threshold)
                                        ++ ")"
                                    )
                                ]
                        )
                        states
                    )
                ]

        Gradient info ->
            div [ class "transition-info" ]
                [ text "Gradient: "
                , span []
                    (List.intersperse (text " -> ")
                        (List.map
                            (\p ->
                                a
                                    [ href "#"
                                    , onClick (SelectPatch p)
                                    ]
                                    [ text p ]
                            )
                            info.patches
                        )
                    )
                , text (" (curve " ++ String.fromFloat info.curve ++ ")")
                ]



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
    div
        [ class
            (if model.selectedPatch == Just name then
                "patch-item selected"

             else
                "patch-item"
            )
        , onClick (SelectPatch name)
        ]
        [ text name ]


transitionPatchNames : HeartbeatInfo -> List String
transitionPatchNames hb =
    case hb.transition of
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
                    div [ class "section patch-editor" ]
                        [ h2 [] [ text patchName ]
                        , div [ class "param-grid" ]
                            (List.map
                                (viewParamSlider patchName patchValues)
                                model.patchParamMeta
                            )
                        ]


viewParamSlider :
    String
    -> Dict String Float
    -> PatchParamMeta
    -> Html Msg
viewParamSlider patchName patchValues meta =
    let
        val =
            Dict.get meta.name patchValues
                |> Maybe.withDefault 0.0
    in
    div [ class "param-slider" ]
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
        , span [ class "param-value" ]
            [ text (formatParamValue val) ]
        ]


formatParamValue : Float -> String
formatParamValue v =
    let
        s =
            String.fromFloat v
    in
    if String.length s > 6 then
        String.left 6 s

    else
        s



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
