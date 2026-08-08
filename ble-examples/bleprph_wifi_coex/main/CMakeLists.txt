idf_component_register(SRCS "main.c" "gatt_svr.c" "tcp_server.c"
                    PRIV_REQUIRES bt nvs_flash esp_timer
                    INCLUDE_DIRS ".")
