extends Node2D

@export
var player_template : PackedScene

# Called when the node enters the scene tree for the first time.
func _ready() -> void:
	GameState.player_joined.connect(add_remote_player)
	$ClientButton.button_down.connect(_on_client_button_button_down)
	$ServerButton.button_down.connect(_on_server_button_pressed)

func add_remote_player(player):
	add_child(player)

# Called every 1/60th of a second
func _physics_process(delta: float) -> void:
	$ClientButton/Label.text = "Amount: " + str(GameState.get_remote_player_amount()) + " Local Player Id: " + GameState.get_local_player_id()
	# both of these won't run if there isn't an active session going
	# so it's fine to put these here
	GameState.poll_client()
	GameState.poll_server()


func _on_server_button_pressed() -> void:
	$ServerButton/Label.text = "Is Running"
	GameState.start_server()
	$TextEdit.text = GameState.get_server_id()


func _on_client_button_button_down() -> void:
	print("starting client")
	var server_iroh_id : String = $TextEdit.text
	var client_player := GameState.start_client(server_iroh_id, player_template)
	if client_player != null:
		add_child(client_player)
	else:
		print("failed to start client")

func _exit_tree() -> void:
	GameState.close_client()
	GameState.close_server()
