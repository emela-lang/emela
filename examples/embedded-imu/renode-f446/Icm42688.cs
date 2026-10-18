using System;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.I2C;

namespace Antmicro.Renode.Peripherals.I2C
{
    // Minimal model of the ICM-42688-P used by the IMU example (bank 0 registers only).
    // run.sh loads it into Renode (include @Icm42688.cs), and Renode compiles it at run time.
    //
    //   - The first byte of a write selects a register; following bytes are written from there on.
    //   - WHO_AM_I (0x75) is 0x47. GYRO_CONFIG0 and ACCEL_CONFIG0 reset to 0x06.
    //   - The data registers (0x1D-0x2A) return fixed physical values (the constants below) as
    //     signed 16-bit values in the range selected by the top 3 bits of ACCEL_CONFIG0 and
    //     GYRO_CONFIG0. While PWR_MGMT0 turns off the accelerometer (bits 1:0) or gyro (bits 3:2),
    //     they return -32768 (0x8000), as on real hardware.
    //   - A read returns everything from the selected register to the end of the registers. The
    //     Renode 1.16.1 STM32F4 I2C controller reads with count=1, so unless the rest comes back
    //     at once, HAL multi-byte receives come up short (same as RegisterSensor in
    //     bareruby's stm32_renode).
    public class Icm42688 : II2CPeripheral
    {
        public Icm42688()
        {
            Reset();
        }

        public void Reset()
        {
            registers = new byte[RegisterCount];
            registers[WhoAmI] = 0x47;
            registers[GyroConfig0] = 0x06;
            registers[AccelConfig0] = 0x06;
            pointer = 0;
        }

        public void Write(byte[] data)
        {
            if(data.Length == 0)
            {
                return;
            }
            pointer = data[0] % RegisterCount;
            for(var i = 1; i < data.Length; i++)
            {
                if(pointer != WhoAmI)
                {
                    registers[pointer] = data[i];
                }
                this.Log(LogLevel.Info, "icm42688 register 0x{0:X2} = 0x{1:X2}", pointer, data[i]);
                pointer = (pointer + 1) % RegisterCount;
            }
        }

        public byte[] Read(int count = 1)
        {
            UpdateData();
            var reply = new byte[RegisterCount - pointer];
            Array.Copy(registers, pointer, reply, 0, reply.Length);
            return reply;
        }

        public void FinishTransmission()
        {
        }

        private void UpdateData()
        {
            var accelOn = (registers[PwrMgmt0] & 0x03) != 0;
            var gyroOn = (registers[PwrMgmt0] & 0x0C) != 0;
            var accelScale = 16 >> (registers[AccelConfig0] >> 5 & 0x03);
            var gyroSelect = registers[GyroConfig0] >> 5;
            var gyroScale = gyroSelect < 4 ? 2000.0 / (1 << gyroSelect) : 125.0 / (1 << (gyroSelect - 4));
            Store(TempData, (int)Math.Round((Celsius - 25.0) * 132.48));
            Store(AccelData, accelOn ? Counts(AccelX, accelScale) : short.MinValue);
            Store(AccelData + 2, accelOn ? Counts(AccelY, accelScale) : short.MinValue);
            Store(AccelData + 4, accelOn ? Counts(AccelZ, accelScale) : short.MinValue);
            Store(GyroData, gyroOn ? Counts(GyroX, gyroScale) : short.MinValue);
            Store(GyroData + 2, gyroOn ? Counts(GyroY, gyroScale) : short.MinValue);
            Store(GyroData + 4, gyroOn ? Counts(GyroZ, gyroScale) : short.MinValue);
        }

        private static int Counts(double value, double fullScale)
        {
            var counts = (int)Math.Round(value * 32768.0 / fullScale);
            return Math.Max(short.MinValue, Math.Min(short.MaxValue, counts));
        }

        private void Store(int register, int value)
        {
            registers[register] = (byte)(value >> 8 & 0xFF);
            registers[register + 1] = (byte)(value & 0xFF);
        }

        private byte[] registers;
        private int pointer;

        private const int RegisterCount = 0x80;
        private const int TempData = 0x1D;
        private const int AccelData = 0x1F;
        private const int GyroData = 0x25;
        private const int PwrMgmt0 = 0x4E;
        private const int GyroConfig0 = 0x4F;
        private const int AccelConfig0 = 0x50;
        private const int WhoAmI = 0x75;

        // Physical values the model measures (g, dps, °C).
        private const double AccelX = 0.25;
        private const double AccelY = -0.5;
        private const double AccelZ = 1.0;
        private const double GyroX = 10.0;
        private const double GyroY = -20.0;
        private const double GyroZ = 0.5;
        private const double Celsius = 30.0;
    }
}
