#include "emela_embedded.h"
#include "emela_board.h"
#include "value.h"
int32_t fixture_answer(void) {
    return fixture_platform_read() + fixture_extra();
}
