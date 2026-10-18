#include "stm32f4xx_hal.h"
#include "emela_embedded.h"

/* NUCLEO-F446RE board for `emela test --device`. Test results are written to Console, which is
 * sent to USART2 (PA2/PA3, the ST-LINK virtual COM port). test.sh runs it in Renode and copies
 * USART2 output to stdout. */

void _init(void) {}
void _fini(void) {}

static UART_HandleTypeDef console;

static void board_fault(void) {
    __disable_irq();
    for (;;) {
        __WFI();
    }
}

static void board_clock(void) {
    RCC_OscInitTypeDef oscillators = {0};
    RCC_ClkInitTypeDef buses = {0};

    __HAL_RCC_PWR_CLK_ENABLE();
    __HAL_PWR_VOLTAGESCALING_CONFIG(PWR_REGULATOR_VOLTAGE_SCALE3);

    oscillators.OscillatorType = RCC_OSCILLATORTYPE_HSI;
    oscillators.HSIState = RCC_HSI_ON;
    oscillators.HSICalibrationValue = RCC_HSICALIBRATION_DEFAULT;
    oscillators.PLL.PLLState = RCC_PLL_ON;
    oscillators.PLL.PLLSource = RCC_PLLSOURCE_HSI;
    oscillators.PLL.PLLM = 16;
    oscillators.PLL.PLLN = 336;
    oscillators.PLL.PLLP = RCC_PLLP_DIV4;
    oscillators.PLL.PLLQ = 2;
    oscillators.PLL.PLLR = 2;
    if (HAL_RCC_OscConfig(&oscillators) != HAL_OK) {
        board_fault();
    }

    buses.ClockType = RCC_CLOCKTYPE_SYSCLK | RCC_CLOCKTYPE_HCLK
                    | RCC_CLOCKTYPE_PCLK1 | RCC_CLOCKTYPE_PCLK2;
    buses.SYSCLKSource = RCC_SYSCLKSOURCE_PLLCLK;
    buses.AHBCLKDivider = RCC_SYSCLK_DIV1;
    buses.APB1CLKDivider = RCC_HCLK_DIV2;
    buses.APB2CLKDivider = RCC_HCLK_DIV1;
    if (HAL_RCC_ClockConfig(&buses, FLASH_LATENCY_2) != HAL_OK) {
        board_fault();
    }
}

static void board_console(void) {
    GPIO_InitTypeDef pins = {0};

    __HAL_RCC_GPIOA_CLK_ENABLE();
    __HAL_RCC_USART2_CLK_ENABLE();
    pins.Pin = GPIO_PIN_2 | GPIO_PIN_3;
    pins.Mode = GPIO_MODE_AF_PP;
    pins.Pull = GPIO_NOPULL;
    pins.Speed = GPIO_SPEED_FREQ_VERY_HIGH;
    pins.Alternate = GPIO_AF7_USART2;
    HAL_GPIO_Init(GPIOA, &pins);

    console.Instance = USART2;
    console.Init.BaudRate = 115200;
    console.Init.WordLength = UART_WORDLENGTH_8B;
    console.Init.StopBits = UART_STOPBITS_1;
    console.Init.Parity = UART_PARITY_NONE;
    console.Init.Mode = UART_MODE_TX_RX;
    console.Init.HwFlowCtl = UART_HWCONTROL_NONE;
    console.Init.OverSampling = UART_OVERSAMPLING_16;
    if (HAL_UART_Init(&console) != HAL_OK) {
        board_fault();
    }
}

void emela_board_init(void) {
    if (HAL_Init() != HAL_OK) {
        board_fault();
    }
    board_clock();
    board_console();
}

void emela_board_console_write(const char *bytes, uint16_t length) {
    HAL_UART_Transmit(&console, (uint8_t *)bytes, length, HAL_MAX_DELAY);
}

void emela_board_console_error_write(const char *bytes, uint16_t length) {
    emela_board_console_write(bytes, length);
}

void emela_board_debug(const char *bytes, uint16_t length) {
    emela_board_console_write("dbg ", 4);
    emela_board_console_write(bytes, length);
    emela_board_console_write("\n", 1);
}

/* The test results are already written to Console. Disable interrupts and stop here. */
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message,
                                  uint16_t length) {
    (void)kind;
    (void)message;
    (void)length;
    __disable_irq();
    for (;;) {
        __WFI();
    }
}

void emela_board_idle(void) {
    __WFI();
}

static uint32_t critical_depth;
static uint32_t critical_primask;

void emela_board_critical_enter(void) {
    uint32_t primask = __get_PRIMASK();
    __disable_irq();
    if (critical_depth++ == 0) {
        critical_primask = primask;
    }
}

void emela_board_critical_exit(void) {
    if (--critical_depth == 0) {
        __set_PRIMASK(critical_primask);
    }
}

int64_t emela_board_now_milliseconds(void) {
    return (int64_t)HAL_GetTick();
}

void emela_board_sleep_milliseconds(int32_t ms) {
    HAL_Delay(ms < 0 ? 0 : (uint32_t)ms);
}

static uint32_t random_state = 2463534242u;

int32_t emela_board_random_int(void) {
    random_state ^= random_state << 13;
    random_state ^= random_state >> 17;
    random_state ^= random_state << 5;
    return (int32_t)random_state;
}

double emela_board_random_float(void) {
    return (double)((uint32_t)emela_board_random_int() >> 8) / 16777216.0;
}

void SysTick_Handler(void) {
    HAL_IncTick();
}
