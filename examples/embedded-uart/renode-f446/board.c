#include "stm32f4xx_hal.h"
#include "emela_embedded.h"

/* Clocks and hooks for NUCLEO-F446RE (UART example). UART operations live in emela_board_hal.c,
 * generated from the board definition. HAL UART uses SysTick for its timeouts, so SysTick
 * keeps running. */

/* Empty startup hook that newlib needs in order to run its init arrays. */
void _init(void) {}
void _fini(void) {}

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

/* Called once before Emela's main. */
void emela_board_init(void) {
    if (HAL_Init() != HAL_OK) {
        board_fault();
    }
    board_clock();
}

/* Emela defect: disable interrupts and stop here. */
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

/* Called repeatedly after main. Waits for an interrupt. */
void emela_board_idle(void) {
    __WFI();
}

void SysTick_Handler(void) {
    HAL_IncTick();
}
