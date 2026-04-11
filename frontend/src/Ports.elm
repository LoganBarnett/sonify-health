port module Ports exposing (consoleError, copyToClipboard, websocketReceive, websocketSend)


port websocketSend : String -> Cmd msg


port consoleError : String -> Cmd msg


port copyToClipboard : String -> Cmd msg


port websocketReceive : (String -> msg) -> Sub msg
