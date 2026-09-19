/*
 * Shared application core for the PIC18F4550 menu demo: a 3-screen LCD
 * control panel driven by epic-taskmgr. One core, two entry points
 * (examples/example_menu_demo.c real hardware, tests/sim_menu_demo.c
 * the mdb gate); only the button source (real RB-change IRQ vs a
 * scripted sim stimulus, via menu_demo_push_event) differs.
 */

#ifndef MENU_DEMO_CORE_H
#define MENU_DEMO_CORE_H

#include <stdint.h>

/** Screens, cycled by SELECT. UP/DOWN only act on MENU_SCREEN_BRIGHTNESS. */
typedef enum {
    MENU_SCREEN_STATUS = 0,   /**< Uptime + live ADC reading. */
    MENU_SCREEN_BRIGHTNESS,   /**< Adjustable 0..10, EEPROM-persisted, drives PWM. */
    MENU_SCREEN_ABOUT,        /**< Tick count + EEPROM write count. */
    MENU_SCREEN_COUNT
} menu_screen_t;

/** Button events, queued from either the real RB-change ISR or a
 *  scripted sim stimulus task; drained by menu_demo_task_ui. */
typedef enum {
    MENU_EVENT_UP = 0,
    MENU_EVENT_DOWN,
    MENU_EVENT_SELECT,
} menu_event_t;

/**
 * @brief Initialize the LCD, ADC, EEPROM-backed settings, and PWM.
 *
 * Call once before spawning any epic-taskmgr task below.
 */
void menu_demo_init(void);

/**
 * @brief Queue a button event (small ring, drops silently if full).
 *
 * Safe to call from ISR context (the real RB-change handler) or from a
 * polled sim-stimulus task.
 *
 * @param ev the event to queue
 */
void menu_demo_push_event(menu_event_t ev);

/**
 * @brief taskmgr task: sample the ADC, redraw if on the status screen.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_adc(void *arg);

/**
 * @brief taskmgr task: drain queued button events, update state, redraw.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_ui(void *arg);

/**
 * @brief taskmgr task: autosave the brightness setting when dirty.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_eeprom(void *arg);

/**
 * @brief taskmgr task: UART heartbeat line + PWM duty update.
 * @param arg unused (epic_taskmgr_fn_t signature)
 */
void menu_demo_task_heartbeat(void *arg);

/**
 * @brief Latest ADC reading, for the sim oracle's cross-checks.
 * @return the last sampled ADC value, 0..1023
 */
uint16_t menu_demo_adc_value(void);

/**
 * @brief Current brightness setting, for the sim oracle's cross-checks.
 * @return the brightness setting, 0..10
 */
uint8_t menu_demo_brightness(void);

/**
 * @brief Current menu screen, for the sim oracle's cross-checks.
 * @return the active screen
 */
menu_screen_t menu_demo_screen(void);

/**
 * @brief Completed EEPROM write count, for the sim oracle's cross-checks.
 * @return the number of completed EEPROM byte writes
 */
uint16_t menu_demo_eeprom_writes(void);

#endif /* MENU_DEMO_CORE_H */
