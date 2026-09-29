(identifier) @variable
(name) @variable

(function_definition name: (name) @function)
(constructor_definition "_init" @function)
(call (identifier) @function)
(attribute_call (identifier) @function)
(signal_statement name: (name) @function)

(type) @type
(type (identifier) @type)
(type (attribute (identifier) @type))
(type (subscript (identifier) @type))
(type (subscript (subscript_arguments (identifier) @type)))
(class_definition name: (name) @type)
(class_name_statement name: (name) @type)
(annotation) @attribute
(annotation (identifier) @attribute)

(string) @string
(string_name) @string
(node_path) @string
(get_node) @string
(escape_sequence) @escape
(integer) @number
(float) @number
[(true) (false) (null)] @constant.builtin
(comment) @comment

[
  "class" "class_name" "extends" "func" "signal" "const" "var"
  "if" "elif" "else" "for" "while" "match" "when" "break" "continue"
  "return" "pass" "await" "export" "onready" "setget" "set" "get"
  (static_keyword)
] @keyword

[
  "and" "or" "not" "as" "is" "in"
  "+" "-" "*" "/" "%" "**" "=" ":=" "==" "!=" "<" ">" "<=" ">="
  "+=" "-=" "*=" "/=" "%=" "**=" "&" "|" "^" "~" "!" "&&" "||"
  "<<" ">>" "&=" "|=" "^=" "<<=" ">>=" "->"
] @operator

["(" ")" "[" "]" "{" "}" ":" "," "." ";"] @punctuation
