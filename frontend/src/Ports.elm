port module Ports exposing (consoleError, websocketReceive, websocketSend)


port websocketSend : String -> Cmd msg


port consoleError : String -> Cmd msg


port websocketReceive : (String -> msg) -> Sub msg
