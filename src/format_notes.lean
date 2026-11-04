import Lean

open Lean Elab Command

-- Calculator notes include fragments such as a trailing equals sign continued
-- by the next box. Preserve these with Lean's Format API.
private def formatNote (env : Environment) (source : String) : CommandElabM String := do
  let doc ← match Parser.runParserCategory env `term source with
    | .ok syntaxTree => liftCoreM <| PrettyPrinter.ppTerm ⟨syntaxTree⟩
    | .error _ =>
      let words := (source.splitOn " ").filter (· != "")
      pure <| Std.Format.group <| Std.Format.joinSep (words.map Std.Format.text) Std.Format.line
  return (doc.pretty 100000).replace "\n" " "

run_cmd do
  let env ← getEnv
  let input ← IO.FS.readFile "lean-input.txt"
  let rows ← (input.splitOn "\n").mapM (formatNote env)
  IO.FS.writeFile "lean-output.txt" (String.intercalate "\n" rows)
