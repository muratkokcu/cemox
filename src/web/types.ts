export type Service = { id: string; name: string };
export type Slot = { start: string; label: string };
export type AvailabilityDay = { date: string; label: string; slots: Slot[] };
export type Availability = {
  timezone: string;
  generatedAt: string;
  rangeStart: string;
  rangeEnd: string;
  days: AvailabilityDay[];
};

export type AppointmentStatus = 'PENDING' | 'APPROVED' | 'REJECTED' | 'EXPIRED' | 'CANCELLED' | 'CONFLICT';
export type Appointment = {
  id: string;
  service_id: string;
  service_name: string;
  start_at: number;
  end_at: number;
  name: string;
  email: string;
  phone: string;
  note: string;
  status: AppointmentStatus;
  hold_expires_at: number;
  created_at: number;
  /** Karara bağlanana kadar null. */
  decision_at: number | null;
  admin_note: string;
};

export type AdminSlot = {
  id: string;
  service_id: string;
  start_at: number;
  end_at: number;
  created_at: number;
};

export type AvailabilityBlock = {
  id: string;
  start_at: number;
  end_at: number;
  reason: string;
};
